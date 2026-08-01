//! A measurement source that is not a device.
//!
//! `leqtion-dsp` can already generate pink noise, white noise, a sine and a
//! sweep — that is how the DSP is tested. This module puts that generator on
//! the *input* side of the ring, so the whole measurement chain (engine,
//! weighting, SPL, LEQ, spectrum, the UI) can be run on a signal whose level is
//! known in advance, on a machine with no interface, no microphone and no
//! microphone permission.
//!
//! ```text
//!   Generator ──push──▶ [ the same ring ] ──pop──▶ analysis thread
//!   (paced by the clock)
//! ```
//!
//! It presents itself as a host with one "device" per signal, so nothing above
//! this crate needs a second code path: the UI enumerates it, opens it and
//! stops it exactly as it does Core Audio.
//!
//! ## What it is and is not for
//!
//! It answers "is the analyser reading what it should?" — pink noise is flat on
//! a constant-percentage-bandwidth display, a full-scale sine reads 0 dBFS, and
//! an LEQ of a steady signal converges on that signal's level. Those are checks
//! you can make anywhere, and they are the ones that catch a broken meter.
//!
//! It cannot answer anything about a *room*, and it is not an acoustic
//! reference: there is no microphone in the chain, so a calibration taken
//! against it would be a number invented out of nothing. `begin_calibration`
//! refuses while this host is open, and that refusal is deliberate.
//!
//! ## Time comes from the clock, not from a driver
//!
//! A real input defines its own time: samples arrive because a converter ran.
//! Here nothing runs, so the thread works out how many samples the elapsed wall
//! time owes and produces exactly that many. Both are "real time" for the
//! purpose of an LEQ, but they fail differently — a driver glitching is a
//! device fault, whereas this drifts only as far as `Instant` does. Samples are
//! counted against elapsed time rather than accumulated per iteration, so a
//! slow iteration is caught up on the next one instead of shortening the
//! measurement.

use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use leqtion_dsp::generator::{Generator, GeneratorConfig, Signal, DEFAULT_LEVEL_DBFS};

use crate::{
    push, AudioError, Capture, CaptureOptions, CaptureStats, DeviceInfo, HostInfo, Result,
    OFFERED_RATES, RING_SECONDS,
};

/// Host id for the generator. Matched case-insensitively, like every other
/// host id, and deliberately not a name any driver would use.
pub const HOST_ID: &str = "Generator";

/// What the UI calls it.
pub const HOST_NAME: &str = "Signal generator";

/// Rate used when nothing asks for one. The generator will run at any offered
/// rate; 48 kHz is the one everything else on the machine is likely to be at.
const DEFAULT_RATE: u32 = 48_000;

/// Samples generated per iteration, at most. Small enough that the ring never
/// waits long for the first block, large enough that the loop is not the cost.
const CHUNK: usize = 2048;

/// How long the thread parks when it is ahead of the clock.
const IDLE: Duration = Duration::from_millis(1);

/// The signals offered, in the order they appear in the input list.
///
/// Pink is first and is the default because it is the one worth looking at: it
/// is flat on a fractional-octave display, so a wrong window, a wrong
/// normalisation or a wrong band integration all show up as a tilt or a step
/// rather than as a plausible-looking curve.
fn signals() -> Vec<(String, Signal)> {
    let at = |what: &str| format!("{what} at {DEFAULT_LEVEL_DBFS:.0} dBFS");
    vec![
        (at("Pink noise"), Signal::Pink),
        (at("White noise"), Signal::White),
        (at("1 kHz sine"), Signal::Sine { hz: 1000.0 }),
        (
            at("Log sweep 20 Hz to 20 kHz"),
            Signal::Sweep {
                from_hz: 20.0,
                to_hz: 20_000.0,
                seconds: 10.0,
            },
        ),
    ]
}

pub fn is_host(id: Option<&str>) -> bool {
    id.is_some_and(|id| id.eq_ignore_ascii_case(HOST_ID))
}

pub fn host() -> HostInfo {
    HostInfo {
        id: HOST_ID.into(),
        name: HOST_NAME.into(),
        available: true,
        // Never the default. Someone who opens LEQtion to measure something
        // should meet their own inputs first.
        is_default: false,
        note: Some(
            "A synthetic source, generated in software. Useful for checking the analyser \
             against a signal whose level is known; it cannot be calibrated, because there \
             is no microphone in the chain."
                .into(),
        ),
    }
}

/// The signals, described as input devices.
///
/// Every offered rate is listed because the generator genuinely runs at all of
/// them — and the difference is worth seeing: the weighting filters are more
/// accurate at 96 kHz than at 44.1, and this is the source that lets someone
/// watch that happen without owning an interface that goes that high.
pub fn devices() -> Vec<DeviceInfo> {
    signals()
        .into_iter()
        .enumerate()
        .map(|(i, (name, _))| DeviceInfo {
            host: HOST_ID.into(),
            name,
            max_channels: 1,
            sample_rates: OFFERED_RATES.to_vec(),
            default_sample_rate: DEFAULT_RATE,
            is_default: i == 0,
        })
        .collect()
}

/// Start generating. Mirrors [`crate::open`]: same handle, same ring, same
/// stop semantics, so the caller cannot tell the two apart.
pub fn open(opts: CaptureOptions) -> Result<(Capture, rtrb::Consumer<f32>)> {
    let available = signals();
    let (name, signal) = match &opts.device {
        Some(want) => available
            .into_iter()
            .find(|(name, _)| name == want)
            .ok_or_else(|| AudioError::UnknownDevice(want.clone()))?,
        None => available
            .into_iter()
            .next()
            .expect("the generator always offers at least one signal"),
    };

    let rate = opts.sample_rate.unwrap_or(DEFAULT_RATE);
    if !OFFERED_RATES.contains(&rate) {
        return Err(AudioError::UnsupportedConfig {
            device: name,
            rate,
            channels: 1,
        });
    }

    let config = GeneratorConfig {
        signal,
        level_dbfs: DEFAULT_LEVEL_DBFS,
        ..GeneratorConfig::default()
    };

    let capacity = (rate as f64 * RING_SECONDS) as usize;
    let (mut producer, consumer) = rtrb::RingBuffer::<f32>::new(capacity);

    let stats = Arc::new(CaptureStats::default());
    let info = crate::StreamInfo {
        host: HOST_ID.into(),
        device: name,
        channels: 1,
        sample_rate: rate,
        // Not a driver format. Saying "F32" alone would suggest a device handed
        // us floats, and the one thing this source must never do is look like
        // an input.
        sample_format: "F32 (generated)".into(),
    };

    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    let thread_stats = Arc::clone(&stats);

    let join = std::thread::Builder::new()
        .name("leqtion-generator".into())
        .spawn(move || {
            let mut generator = Generator::new(config, rate as f64);
            let mut buf = [0.0f32; CHUNK];
            let started = Instant::now();
            let mut produced: u64 = 0;

            thread_stats.running.store(true, Ordering::Relaxed);

            loop {
                match stop_rx.recv_timeout(IDLE) {
                    Ok(()) | Err(RecvTimeoutError::Disconnected) => break,
                    Err(RecvTimeoutError::Timeout) => {}
                }

                // Owed, not "one buffer per tick": the debt is measured against
                // the clock, so an iteration that ran late is made up rather
                // than quietly losing the samples it did not produce.
                let due = (started.elapsed().as_secs_f64() * rate as f64) as u64;
                while produced < due {
                    let want = (due - produced).min(CHUNK as u64) as usize;
                    generator.fill(&mut buf[..want]);
                    // Counts a full ring as dropped frames, exactly as the
                    // device path does. The samples existed for as long as the
                    // clock says they did; if the analysis thread could not take
                    // them, time is missing from the measurement and every LEQ
                    // on screen is short. That is the same failure either way.
                    push(&mut producer, &buf[..want], &thread_stats);
                    produced += want as u64;
                }
            }

            thread_stats.running.store(false, Ordering::Relaxed);
        })
        .map_err(|e| AudioError::Backend(e.to_string()))?;

    Ok((
        Capture {
            stop: Some(stop_tx),
            join: Some(join),
            info,
            stats,
        },
        consumer,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_generator_host_is_recognised_by_id() {
        assert!(is_host(Some(HOST_ID)));
        assert!(is_host(Some("generator")));
        assert!(!is_host(Some("CoreAudio")));
        assert!(!is_host(None));
    }

    #[test]
    fn every_signal_is_offered_as_a_device_that_can_be_opened() {
        let devices = devices();
        assert_eq!(devices.len(), signals().len());
        assert!(devices.iter().filter(|d| d.is_default).count() == 1);

        for device in devices {
            let opts = CaptureOptions {
                host: Some(HOST_ID.into()),
                device: Some(device.name.clone()),
                sample_rate: Some(48_000),
                buffer_frames: None,
            };
            let (capture, _consumer) = open(opts).expect("the generator must open");
            assert_eq!(capture.info.device, device.name);
            assert_eq!(capture.info.channels, 1);
            capture.stop();
        }
    }

    #[test]
    fn a_signal_that_does_not_exist_is_an_error_not_a_panic() {
        let opts = CaptureOptions {
            host: Some(HOST_ID.into()),
            device: Some("Brown noise".into()),
            ..Default::default()
        };
        assert!(matches!(open(opts), Err(AudioError::UnknownDevice(_))));
    }

    #[test]
    fn a_rate_nothing_offers_is_refused_rather_than_silently_substituted() {
        let opts = CaptureOptions {
            host: Some(HOST_ID.into()),
            device: None,
            sample_rate: Some(22_050),
            buffer_frames: None,
        };
        assert!(matches!(
            open(opts),
            Err(AudioError::UnsupportedConfig { .. })
        ));
    }

    /// The point of the whole module: samples arrive, at the rate the clock
    /// says they should, without a device.
    #[test]
    fn samples_arrive_in_real_time() {
        let rate = 48_000u32;
        let opts = CaptureOptions {
            host: Some(HOST_ID.into()),
            device: None,
            sample_rate: Some(rate),
            buffer_frames: None,
        };
        let (capture, mut consumer) = open(opts).expect("the generator must open");

        std::thread::sleep(Duration::from_millis(300));
        let produced = consumer.slots();
        capture.stop();

        // Generous either side: this asserts that time is being tracked at all,
        // not that a test machine under load schedules a thread promptly.
        let expected = rate as f64 * 0.3;
        assert!(
            produced as f64 > expected * 0.5 && (produced as f64) < expected * 2.0,
            "{produced} samples in 300 ms at {rate} Hz, expected about {expected}"
        );

        // And they are the signal, not silence.
        let chunk = consumer.read_chunk(produced).expect("samples are readable");
        let (first, _) = chunk.as_slices();
        assert!(
            first.iter().any(|s| s.abs() > 1e-4),
            "the generator produced only silence"
        );
    }
}
