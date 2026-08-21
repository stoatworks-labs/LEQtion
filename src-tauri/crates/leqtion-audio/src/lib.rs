//! Audio capture.
//!
//! Two jobs: tell the UI what inputs exist, and get samples from a driver
//! callback to the analysis thread without ever blocking the driver.
//!
//! ```text
//!   driver callback ──push──▶ [ lock-free SPSC ring ] ──pop──▶ analysis thread
//!        (real-time)                                              (ordinary)
//! ```
//!
//! ## The rule that shapes this whole file
//!
//! **The audio callback must not allocate, lock, or block.** A `Mutex` around
//! the engine would work almost all the time and then, on the night, invert
//! priority against a UI thread mid-repaint and drop a buffer. So the callback
//! does exactly one thing — copy interleaved samples into a ring — and the
//! analysis happens on a thread that is allowed to be slow. If the ring
//! overflows, the count is recorded and the samples are dropped; a measurement
//! that quietly stretches time would be worse than one that admits a gap.
//!
//! ## Why a thread owns the stream
//!
//! `cpal::Stream` is not `Send` on every platform, so it cannot be stored in
//! shared application state and stopped from wherever a command happens to run.
//! Instead a dedicated thread builds the stream, keeps it alive, and waits for a
//! stop signal. That also gives one obvious place for the driver's error
//! callback to report to.
//!
//! ## The one input that is not a device
//!
//! [`synthetic`] presents the DSP crate's signal generator as a host with one
//! device per signal, so a measurement can be run against a known level on a
//! machine with no interface. It fills the same ring and returns the same
//! [`Capture`], which is why `hosts`, `devices` and `open` are the only three
//! places in the codebase that know it is not real.

#[cfg(target_os = "android")]
pub mod android;
pub mod profiles;
pub mod session;
pub mod synthetic;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("no audio host named {0}")]
    UnknownHost(String),
    #[error("no input device named {0}")]
    UnknownDevice(String),
    #[error("no output device named {0}")]
    UnknownOutputDevice(String),
    #[error("the host has no input devices")]
    NoDevices,
    #[error("{device} cannot run at {rate} Hz with {channels} channels")]
    UnsupportedConfig {
        device: String,
        rate: u32,
        channels: u16,
    },
    #[error("audio backend: {0}")]
    Backend(String),
}

type Result<T> = std::result::Result<T, AudioError>;

/// An audio API — CoreAudio, WASAPI, ALSA, JACK, ASIO.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostInfo {
    /// Stable identifier used to reopen this host later.
    pub id: String,
    /// What to show a user.
    pub name: String,
    /// True if this build can actually use it. ASIO is listed even when it was
    /// not compiled in, so the UI can explain its absence rather than leave the
    /// user wondering where their interface went.
    pub available: bool,
    pub is_default: bool,
    /// Why it is unavailable, when it is.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceInfo {
    pub host: String,
    pub name: String,
    /// Highest channel count the device offers for input.
    pub max_channels: u16,
    /// Sample rates worth offering, filtered to the ones a measurement would
    /// use and sorted ascending.
    pub sample_rates: Vec<u32>,
    pub default_sample_rate: u32,
    pub is_default: bool,
}

/// Rates offered in the UI, if the device supports them. Anything below 44.1 kHz
/// cannot carry the top of the measurement range, and anything above 192 kHz
/// costs CPU for spectrum nobody is measuring.
const OFFERED_RATES: [u32; 6] = [44100, 48000, 88200, 96000, 176400, 192000];

/// Identify the ASIO host across cpal versions without depending on the enum
/// variant existing in this build.
fn host_id_name(id: &cpal::HostId) -> String {
    format!("{id:?}")
}

/// Every audio API this build knows about.
///
/// Hosts that were not compiled in still appear, marked unavailable with a
/// reason. Silently omitting ASIO on a Windows build that lacks it produces a
/// support question every single time.
pub fn hosts() -> Vec<HostInfo> {
    let default = cpal::default_host().id();
    let mut out: Vec<HostInfo> = cpal::available_hosts()
        .into_iter()
        .map(|id| {
            let name = host_id_name(&id);
            HostInfo {
                id: name.clone(),
                name: friendly_host_name(&name),
                available: true,
                is_default: id == default,
                note: None,
            }
        })
        .collect();

    if cfg!(target_os = "windows") && !out.iter().any(|h| h.id.eq_ignore_ascii_case("asio")) {
        out.push(HostInfo {
            id: "Asio".into(),
            name: "ASIO".into(),
            available: false,
            is_default: false,
            note: Some(
                "This build was compiled without ASIO. It needs the Steinberg ASIO SDK, \
                 which cannot be redistributed — see docs/asio.md."
                    .into(),
            ),
        });
    }

    // Last, and never the default: a tool for checking the analyser, not an
    // input anyone came here to measure.
    out.push(synthetic::host());

    out
}

fn friendly_host_name(id: &str) -> String {
    match id.to_ascii_lowercase().as_str() {
        "coreaudio" => "Core Audio".into(),
        "wasapi" => "WASAPI".into(),
        "asio" => "ASIO".into(),
        "alsa" => "ALSA".into(),
        "jack" => "JACK".into(),
        other => other.to_string(),
    }
}

fn find_host(id: Option<&str>) -> Result<cpal::Host> {
    let Some(id) = id else {
        return Ok(cpal::default_host());
    };
    for candidate in cpal::available_hosts() {
        if host_id_name(&candidate).eq_ignore_ascii_case(id) {
            return cpal::host_from_id(candidate)
                .map_err(|e| AudioError::Backend(e.to_string()));
        }
    }
    Err(AudioError::UnknownHost(id.to_string()))
}

/// Input devices on a host, or on the default host if none is named.
pub fn devices(host_id: Option<&str>) -> Result<Vec<DeviceInfo>> {
    if synthetic::is_host(host_id) {
        return Ok(synthetic::devices());
    }
    let host = find_host(host_id)?;
    let host_name = host_id_name(&host.id());
    let default_name = host.default_input_device().and_then(|d| d.description().ok()).map(|d| d.name().to_string());

    let devices = host
        .input_devices()
        .map_err(|e| AudioError::Backend(e.to_string()))?;

    let mut out = Vec::new();
    for device in devices {
        let Ok(name) = device.description().map(|d| d.name().to_string()) else {
            continue;
        };

        // A device that refuses to describe itself is not a device we can open.
        // Skipping it beats listing something that fails on selection.
        let Ok(configs) = device.supported_input_configs() else {
            continue;
        };
        let configs: Vec<_> = configs.collect();
        if configs.is_empty() {
            continue;
        }

        let max_channels = configs.iter().map(|c| c.channels()).max().unwrap_or(0);
        let mut sample_rates: Vec<u32> = OFFERED_RATES
            .iter()
            .copied()
            .filter(|&r| {
                configs.iter().any(|c| {
                    c.min_sample_rate() <= r && r <= c.max_sample_rate()
                })
            })
            .collect();

        let default_sample_rate = device
            .default_input_config()
            .map(|c| c.sample_rate())
            .unwrap_or(48000);
        if !sample_rates.contains(&default_sample_rate) {
            sample_rates.push(default_sample_rate);
        }
        sample_rates.sort_unstable();
        sample_rates.dedup();

        out.push(DeviceInfo {
            host: host_name.clone(),
            name: name.clone(),
            max_channels,
            sample_rates,
            default_sample_rate,
            is_default: default_name.as_deref() == Some(name.as_str()),
        });
    }

    if out.is_empty() {
        return Err(AudioError::NoDevices);
    }
    Ok(out)
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureOptions {
    #[serde(default)]
    pub host: Option<String>,
    #[serde(default)]
    pub device: Option<String>,
    #[serde(default)]
    pub sample_rate: Option<u32>,
    /// Driver buffer size in frames. `None` leaves it to the driver, which is
    /// the right default — a measurement does not care about latency, and
    /// forcing a small buffer only invites dropouts.
    #[serde(default)]
    pub buffer_frames: Option<u32>,
}

/// What a running capture actually settled on, which is not always what was
/// asked for.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamInfo {
    pub host: String,
    pub device: String,
    pub channels: u16,
    pub sample_rate: u32,
    /// The sample format the driver gave us, before conversion to f32.
    pub sample_format: String,
}

/// Counters a UI can show to explain a measurement that looks wrong.
#[derive(Debug, Default)]
pub struct CaptureStats {
    /// Frames dropped because the analysis thread fell behind. Any non-zero
    /// value invalidates an LEQ, because time went missing.
    pub dropped_frames: AtomicU64,
    /// Errors reported by the driver.
    pub stream_errors: AtomicU64,
    pub running: AtomicBool,
}

/// A running capture. Dropping it stops the stream.
pub struct Capture {
    stop: Option<mpsc::Sender<()>>,
    join: Option<JoinHandle<()>>,
    pub info: StreamInfo,
    pub stats: Arc<CaptureStats>,
}

impl Capture {
    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        if let Some(tx) = self.stop.take() {
            let _ = tx.send(());
        }
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// How much audio the ring holds. Two seconds is far more than the analysis
/// thread should ever need, and the memory is trivial; the point is that a
/// hitch — a slow repaint, a device change — costs nothing rather than a gap.
const RING_SECONDS: f64 = 2.0;

/// Open an input.
///
/// Returns the capture handle and the consumer end of the ring. Pop from the
/// consumer on a normal thread and feed the samples to the engine; the values
/// are interleaved, `info.channels` per frame.
pub fn open(opts: CaptureOptions) -> Result<(Capture, rtrb::Consumer<f32>)> {
    if synthetic::is_host(opts.host.as_deref()) {
        return synthetic::open(opts);
    }

    // iOS will not open an input until an audio session permitting it is
    // active, and the mode set here is what keeps AGC out of the measurement.
    // A no-op everywhere else. See `session`.
    session::prepare_for_measurement().map_err(AudioError::Backend)?;

    let host = find_host(opts.host.as_deref())?;
    let host_name = host_id_name(&host.id());

    let device = match &opts.device {
        Some(want) => host
            .input_devices()
            .map_err(|e| AudioError::Backend(e.to_string()))?
            .find(|d| d.description().map(|d| d.name() == want).unwrap_or(false))
            .ok_or_else(|| AudioError::UnknownDevice(want.clone()))?,
        None => host.default_input_device().ok_or(AudioError::NoDevices)?,
    };
    let device_name = device
        .description()
        .map(|d| d.name().to_string())
        .unwrap_or_else(|_| "unknown".into());

    let default_config = device
        .default_input_config()
        .map_err(|e| AudioError::Backend(e.to_string()))?;
    let sample_format = default_config.sample_format();
    let channels = default_config.channels();

    let rate = opts.sample_rate.unwrap_or(default_config.sample_rate());
    let supported = device
        .supported_input_configs()
        .map_err(|e| AudioError::Backend(e.to_string()))?
        .any(|c| {
            c.channels() == channels
                && c.min_sample_rate() <= rate
                && rate <= c.max_sample_rate()
        });
    if !supported {
        return Err(AudioError::UnsupportedConfig {
            device: device_name,
            rate,
            channels,
        });
    }

    let mut config: cpal::StreamConfig = default_config.into();
    config.sample_rate = rate;
    if let Some(frames) = opts.buffer_frames {
        config.buffer_size = cpal::BufferSize::Fixed(frames);
    }

    let capacity = (rate as f64 * RING_SECONDS) as usize * channels as usize;
    let (producer, consumer) = rtrb::RingBuffer::<f32>::new(capacity);

    let stats = Arc::new(CaptureStats::default());
    let info = StreamInfo {
        host: host_name,
        device: device_name,
        channels,
        sample_rate: rate,
        sample_format: format!("{sample_format:?}"),
    };

    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    let (ready_tx, ready_rx) = mpsc::channel::<std::result::Result<(), String>>();

    let thread_stats = Arc::clone(&stats);
    let join = std::thread::Builder::new()
        .name("leqtion-audio".into())
        .spawn(move || {
            run_stream(
                device,
                config,
                sample_format,
                producer,
                thread_stats,
                ready_tx,
                stop_rx,
            );
        })
        .map_err(|e| AudioError::Backend(e.to_string()))?;

    // Wait for the stream to actually start, so a failure surfaces as an error
    // from `open` rather than as a meter that never moves.
    match ready_rx.recv_timeout(Duration::from_secs(10)) {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            let _ = join.join();
            return Err(AudioError::Backend(e));
        }
        Err(_) => {
            let _ = stop_tx.send(());
            let _ = join.join();
            return Err(AudioError::Backend(
                "the audio device did not start within ten seconds".into(),
            ));
        }
    }

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

#[allow(clippy::too_many_arguments)]
fn run_stream(
    device: cpal::Device,
    config: cpal::StreamConfig,
    format: cpal::SampleFormat,
    producer: rtrb::Producer<f32>,
    stats: Arc<CaptureStats>,
    ready: mpsc::Sender<std::result::Result<(), String>>,
    stop: mpsc::Receiver<()>,
) {
    let err_stats = Arc::clone(&stats);
    let on_error = move |e: cpal::Error| {
        err_stats.stream_errors.fetch_add(1, Ordering::Relaxed);
        tracing::error!("audio stream error: {e}");
    };

    let build = build_stream(&device, &config, format, producer, Arc::clone(&stats), on_error);

    let stream = match build {
        Ok(s) => s,
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };

    if let Err(e) = stream.play() {
        let _ = ready.send(Err(e.to_string()));
        return;
    }

    stats.running.store(true, Ordering::Relaxed);
    let _ = ready.send(Ok(()));

    // Hold the stream alive on this thread until asked to stop. `recv` rather
    // than a sleep loop: nothing to poll, and the thread costs nothing parked.
    loop {
        match stop.recv_timeout(Duration::from_millis(500)) {
            Ok(()) | Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {}
        }
    }

    stats.running.store(false, Ordering::Relaxed);
    drop(stream);
}

/// Build the input stream for whichever sample format the driver hands us.
///
/// cpal is generic over the sample type, so each format needs its own callback.
/// The bodies are identical apart from the conversion to f32, which
/// `cpal::Sample` does losslessly for every integer format.
fn build_stream<E>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    format: cpal::SampleFormat,
    mut producer: rtrb::Producer<f32>,
    stats: Arc<CaptureStats>,
    on_error: E,
) -> std::result::Result<cpal::Stream, String>
where
    E: FnMut(cpal::Error) + Send + 'static,
{
    macro_rules! stream_for {
        ($sample:ty) => {{
            device
                .build_input_stream(
                    config.clone(),
                    move |data: &[$sample], _: &cpal::InputCallbackInfo| {
                        push(&mut producer, data, &stats);
                    },
                    on_error,
                    None,
                )
                .map_err(|e| e.to_string())
        }};
    }

    match format {
        cpal::SampleFormat::F32 => stream_for!(f32),
        cpal::SampleFormat::I16 => stream_for!(i16),
        cpal::SampleFormat::U16 => stream_for!(u16),
        cpal::SampleFormat::I32 => stream_for!(i32),
        cpal::SampleFormat::I8 => stream_for!(i8),
        cpal::SampleFormat::U8 => stream_for!(u8),
        cpal::SampleFormat::F64 => stream_for!(f64),
        other => Err(format!("unsupported sample format {other:?}")),
    }
}

/// The whole of the real-time path: convert and copy, count what will not fit.
///
/// No allocation, no locking, no logging. `chunks` on the ring is the only
/// slightly awkward part and it is worth it — writing sample by sample through
/// `push` would be several times slower for no benefit.
#[inline]
fn push<S>(producer: &mut rtrb::Producer<f32>, data: &[S], stats: &CaptureStats)
where
    S: cpal::Sample + cpal::SizedSample,
    f32: cpal::FromSample<S>,
{
    let room = producer.slots();
    let take = room.min(data.len());

    if take < data.len() {
        stats
            .dropped_frames
            .fetch_add((data.len() - take) as u64, Ordering::Relaxed);
    }
    if take == 0 {
        return;
    }

    if let Ok(mut chunk) = producer.write_chunk_uninit(take) {
        let (first, second) = chunk.as_mut_slices();
        let mut i = 0;
        for slot in first.iter_mut() {
            slot.write(<f32 as cpal::FromSample<S>>::from_sample_(data[i]));
            i += 1;
        }
        for slot in second.iter_mut() {
            slot.write(<f32 as cpal::FromSample<S>>::from_sample_(data[i]));
            i += 1;
        }
        // Safety: exactly `take` slots were written above, and `take` is the
        // length the chunk was requested with.
        unsafe { chunk.commit_all() };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Enumeration must not panic on whatever this machine happens to have, and
    /// must always name a default host.
    #[test]
    fn hosts_can_be_listed() {
        let hosts = hosts();
        assert!(!hosts.is_empty(), "no audio hosts at all");
        assert!(
            hosts.iter().any(|h| h.is_default && h.available),
            "no available default host in {hosts:?}"
        );
    }

    #[test]
    fn every_host_has_a_friendly_name() {
        for h in hosts() {
            assert!(!h.name.is_empty());
            assert!(!h.id.is_empty());
            if !h.available {
                assert!(h.note.is_some(), "{} is unavailable with no reason", h.name);
            }
        }
    }

    #[test]
    fn an_unknown_host_is_an_error_not_a_panic() {
        assert!(matches!(
            find_host(Some("definitely-not-a-host")),
            Err(AudioError::UnknownHost(_))
        ));
        assert!(devices(Some("definitely-not-a-host")).is_err());
    }

    /// Device enumeration is allowed to find nothing — a CI runner has no
    /// microphone — but it must not fail in any other way, and anything it does
    /// report must be openable in principle.
    #[test]
    fn devices_are_described_consistently() {
        match devices(None) {
            Ok(list) => {
                for d in list {
                    assert!(!d.name.is_empty());
                    assert!(d.max_channels > 0, "{} claims no channels", d.name);
                    assert!(
                        !d.sample_rates.is_empty(),
                        "{} offers no sample rates",
                        d.name
                    );
                    assert!(
                        d.sample_rates.contains(&d.default_sample_rate),
                        "{} defaults to {} which is not in its own list {:?}",
                        d.name,
                        d.default_sample_rate,
                        d.sample_rates
                    );
                    assert!(d.sample_rates.windows(2).all(|w| w[0] < w[1]));
                }
            }
            Err(AudioError::NoDevices) => {}
            Err(e) => panic!("enumeration failed unexpectedly: {e}"),
        }
    }

    #[test]
    fn opening_a_device_that_does_not_exist_fails_cleanly() {
        let err = open(CaptureOptions {
            device: Some("no such interface".into()),
            ..Default::default()
        });
        assert!(matches!(err, Err(AudioError::UnknownDevice(_)) | Err(AudioError::NoDevices)));
    }
}

// ---------------------------------------------------------------------------
// Output
// ---------------------------------------------------------------------------

/// Which output to open, and where on it to put the signal.
///
/// Deliberately no `host` field: the output always opens on the **same host and
/// device as the input**. Two unsynchronised converters drift apart by
/// milliseconds over a few minutes, and a transfer function measured across
/// that drift shows a phase trace rotating steadily — which looks exactly like
/// a real system fault and is not one. Sharing the device shares the clock.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputOptions {
    /// Host id, which must match the input's.
    pub host: Option<String>,
    /// Device name. `None` opens the default output.
    pub device: Option<String>,
    /// Must match the input's rate, for the same reason.
    pub sample_rate: Option<u32>,
    pub buffer_frames: Option<u32>,
}

/// A running output. Dropping it stops the stream.
pub struct Output {
    stop: Option<mpsc::Sender<()>>,
    join: Option<JoinHandle<()>>,
    pub info: StreamInfo,
    pub stats: Arc<CaptureStats>,
}

impl Output {
    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        if let Some(tx) = self.stop.take() {
            let _ = tx.send(());
        }
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl Drop for Output {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Open an output and drive it from `fill`.
///
/// `fill` is called on the audio thread with an interleaved buffer and the
/// channel count, and must obey the same rule as everything else on that
/// thread: no allocation, no locking, no blocking. It is expected to write
/// every sample, including the channels it is not using — see
/// `Generator::fill_interleaved` for why silence has to be written rather than
/// left alone.
pub fn open_output<F>(opts: OutputOptions, mut fill: F) -> Result<Output>
where
    F: FnMut(&mut [f32], usize) + Send + 'static,
{
    // The generator plays out while an input is open, which is why the session
    // category is playAndRecord rather than record. A no-op off iOS.
    session::prepare_for_measurement().map_err(AudioError::Backend)?;

    let host = find_host(opts.host.as_deref())?;
    let host_name = host_id_name(&host.id());

    let device = match &opts.device {
        Some(want) => host
            .output_devices()
            .map_err(|e| AudioError::Backend(e.to_string()))?
            .find(|d| d.description().map(|d| d.name() == want).unwrap_or(false))
            .ok_or_else(|| AudioError::UnknownOutputDevice(want.clone()))?,
        None => host.default_output_device().ok_or(AudioError::NoDevices)?,
    };
    let device_name = device
        .description()
        .map(|d| d.name().to_string())
        .unwrap_or_else(|_| "unknown".into());

    let default_config = device
        .default_output_config()
        .map_err(|e| AudioError::Backend(e.to_string()))?;
    let sample_format = default_config.sample_format();
    let channels = default_config.channels();
    let rate = opts.sample_rate.unwrap_or(default_config.sample_rate());

    let supported = device
        .supported_output_configs()
        .map_err(|e| AudioError::Backend(e.to_string()))?
        .any(|c| {
            c.channels() == channels && c.min_sample_rate() <= rate && rate <= c.max_sample_rate()
        });
    if !supported {
        return Err(AudioError::UnsupportedConfig {
            device: device_name,
            rate,
            channels,
        });
    }

    let mut config: cpal::StreamConfig = default_config.into();
    config.sample_rate = rate;
    if let Some(frames) = opts.buffer_frames {
        config.buffer_size = cpal::BufferSize::Fixed(frames);
    }

    let stats = Arc::new(CaptureStats::default());
    let info = StreamInfo {
        host: host_name,
        device: device_name,
        channels,
        sample_rate: rate,
        sample_format: format!("{sample_format:?}"),
    };

    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    let (ready_tx, ready_rx) = mpsc::channel::<std::result::Result<(), String>>();
    let thread_stats = Arc::clone(&stats);
    let chans = channels as usize;

    let join = std::thread::Builder::new()
        .name("leqtion-output".into())
        .spawn(move || {
            let err_stats = Arc::clone(&thread_stats);
            let on_error = move |e: cpal::Error| {
                err_stats.stream_errors.fetch_add(1, Ordering::Relaxed);
                tracing::error!("output stream error: {e}");
            };

            // Only f32 is built here. Every host cpal supports offers an f32
            // output config, and generating into an integer format would mean
            // dithering decisions that a measurement signal generator has no
            // business making silently.
            let built = device.build_output_stream(
                config,
                move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                    fill(data, chans);
                },
                on_error,
                None,
            );

            let stream = match built {
                Ok(s) => s,
                Err(e) => {
                    let _ = ready_tx.send(Err(e.to_string()));
                    return;
                }
            };
            if let Err(e) = stream.play() {
                let _ = ready_tx.send(Err(e.to_string()));
                return;
            }

            thread_stats.running.store(true, Ordering::Relaxed);
            let _ = ready_tx.send(Ok(()));

            loop {
                match stop_rx.recv_timeout(Duration::from_millis(500)) {
                    Ok(()) | Err(RecvTimeoutError::Disconnected) => break,
                    Err(RecvTimeoutError::Timeout) => {}
                }
            }
            thread_stats.running.store(false, Ordering::Relaxed);
            drop(stream);
        })
        .map_err(|e| AudioError::Backend(e.to_string()))?;

    match ready_rx.recv_timeout(Duration::from_secs(10)) {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            let _ = join.join();
            return Err(AudioError::Backend(e));
        }
        Err(_) => {
            let _ = stop_tx.send(());
            let _ = join.join();
            return Err(AudioError::Backend(
                "the output device did not start within ten seconds".into(),
            ));
        }
    }

    Ok(Output {
        stop: Some(stop_tx),
        join: Some(join),
        info,
        stats,
    })
}

/// Output devices on a host, for the generator's routing picker.
pub fn output_devices(host_id: Option<&str>) -> Result<Vec<DeviceInfo>> {
    let host = find_host(host_id)?;
    let host_name = host_id_name(&host.id());
    let default_name = host
        .default_output_device()
        .and_then(|d| d.description().ok())
        .map(|d| d.name().to_string());

    let mut out = Vec::new();
    for device in host
        .output_devices()
        .map_err(|e| AudioError::Backend(e.to_string()))?
    {
        let Ok(name) = device.description().map(|d| d.name().to_string()) else {
            continue;
        };
        let Ok(configs) = device.supported_output_configs() else {
            continue;
        };
        let configs: Vec<_> = configs.collect();
        if configs.is_empty() {
            continue;
        }
        let max_channels = configs.iter().map(|c| c.channels()).max().unwrap_or(0);
        let default_sample_rate = device
            .default_output_config()
            .map(|c| c.sample_rate())
            .unwrap_or(48000);
        let mut sample_rates: Vec<u32> = OFFERED_RATES
            .iter()
            .copied()
            .filter(|&r| {
                configs
                    .iter()
                    .any(|c| c.min_sample_rate() <= r && r <= c.max_sample_rate())
            })
            .collect();
        if !sample_rates.contains(&default_sample_rate) {
            sample_rates.push(default_sample_rate);
        }
        sample_rates.sort_unstable();
        sample_rates.dedup();

        out.push(DeviceInfo {
            host: host_name.clone(),
            name: name.clone(),
            max_channels,
            sample_rates,
            default_sample_rate,
            is_default: default_name.as_deref() == Some(name.as_str()),
        });
    }

    if out.is_empty() {
        return Err(AudioError::NoDevices);
    }
    Ok(out)
}
