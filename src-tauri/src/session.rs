//! The running measurement: an open input, an optional output, an analysis
//! thread, and the engines they feed.
//!
//! ```text
//!   input  ──▶ ring ──┐
//!                     ├──▶ analysis thread ──▶ Engine        ──┐
//!   generator ──▶ ring┘                    └─▶ TransferFunction┴─▶ frame ──▶ UI
//!        ▲
//!        └── output callback
//! ```
//!
//! The analysis thread is the only thing that pushes samples into either
//! engine, and it does so on every sample it can get. Frames go out to the UI on
//! a timer *inside* the same loop rather than from a separate ticker, so a UI
//! that stops listening can never make the engines skip audio: integration is
//! driven by samples, display by the clock.
//!
//! ## Keeping the two channels aligned
//!
//! A transfer function needs the reference and the measurement to be the same
//! instant of time. Two cases, and they fail differently:
//!
//! - **Hardware loopback** — both channels come out of the same interleaved
//!   input block, so they are aligned by construction and nothing can drift.
//! - **Internal tap** — the reference is what the generator produced, read from
//!   its own ring. Input and output share a device and therefore a clock, so the
//!   *rates* match exactly, but the starting offset is arbitrary and is the
//!   round trip through the converters, the system and the air. That is what the
//!   delay finder measures.
//!
//! For the internal tap the loop always consumes exactly as many reference
//! samples as there are measurement frames. If the tap runs dry it is padded
//! with silence and counted — not skipped, because skipping would shift the
//! alignment permanently and every phase reading afterwards would be wrong with
//! nothing on screen to say so.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use leqtion_audio::{Capture, CaptureOptions, Output, OutputOptions, StreamInfo};
use crate::logger::Logger;
use leqtion_dsp::engine::{fold_channels, Engine, EngineConfig, Frame};
use leqtion_dsp::generator::{Generator, GeneratorConfig};
use leqtion_dsp::transfer::{TransferConfig, TransferFunction, TransferReading};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};

/// Event name the UI listens on for frames.
pub const FRAME_EVENT: &str = "leqtion://frame";

/// Display rate.
const FRAME_INTERVAL: Duration = Duration::from_millis(33);
/// How long the analysis thread sleeps when the input ring is empty.
const IDLE_SLEEP: Duration = Duration::from_millis(2);
/// Samples pulled from the input ring per iteration.
const CHUNK: usize = 4096;

/// Where the transfer function's reference signal comes from.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ReferenceSource {
    /// No transfer function. The meter runs on its own.
    #[default]
    Off,
    /// The generator's own output, tapped before it reaches the converter.
    ///
    /// Costs no input channel and no cable, and is right for measuring a room
    /// or a processor chain. It cannot see the output converter or anything
    /// upstream of where the signal actually leaves the machine.
    Internal,
    /// A physical input carrying a loopback of the output.
    ///
    /// The rigorous choice: the reference is what genuinely left the interface,
    /// so the output converter and any output processing are inside the
    /// measurement rather than assumed transparent.
    Loopback { channel: usize },
}

/// Everything the analysis thread owns, behind one lock.
///
/// One mutex rather than one per engine: the thread takes it once per chunk and
/// the commands take it briefly, so there is nothing to be gained from finer
/// granularity and a great deal to be lost from two locks that must be taken in
/// the right order.
pub struct Analysis {
    pub engine: Engine,
    pub transfer: TransferFunction,
    pub reference: ReferenceSource,
    /// The open log, if one is running.
    ///
    /// Kept here rather than beside the session so it is behind the same lock as
    /// the engine: a row is written from the engine's own history in the same
    /// critical section that produced it, which is what makes "the log is the
    /// chart" true rather than nearly true.
    pub log: Option<Logger>,
    /// Cumulative dropped frames as last reported by the capture, so a row can
    /// carry it without the analysis thread reaching for the session lock.
    pub dropped_frames: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionStatus {
    pub running: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<StreamInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<StreamInfo>,
    pub dropped_frames: u64,
    pub stream_errors: u64,
    /// Reference samples that were not available when the measurement needed
    /// them. Any non-zero value means the internal tap lost alignment and the
    /// delay must be found again.
    pub reference_underruns: u64,
    pub reference: ReferenceSource,
    /// True when input and output are the same device, and therefore the same
    /// clock. False means the internal reference will drift and the delay has to
    /// be found again periodically — see `Session::start`.
    pub clock_shared: bool,
}

/// What the audio thread is told when the generator changes.
///
/// The output channel travels with the settings rather than being captured when
/// the stream opens, so moving the signal from output 1 to output 7 does not
/// mean tearing the device down and back up — which on some interfaces is a
/// second of silence and an audible relay click.
#[derive(Debug, Clone, Copy)]
struct GeneratorCommand {
    config: GeneratorConfig,
    channel: usize,
}

/// The payload sent to the UI: the meter frame, with the transfer function
/// flattened alongside it when one is running.
#[derive(Clone, Serialize)]
struct FramePayload {
    #[serde(flatten)]
    meter: Frame,
    #[serde(skip_serializing_if = "Option::is_none")]
    transfer: Option<TransferReading>,
}

pub struct Session {
    analysis: Arc<Mutex<Analysis>>,
    capture: Option<Capture>,
    output: Option<Output>,
    generator_tx: Option<std::sync::mpsc::Sender<GeneratorCommand>>,
    generator_config: GeneratorConfig,
    /// Which output channel the generator drives.
    generator_channel: usize,
    reference_underruns: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    stream: Option<StreamInfo>,
    output_info: Option<StreamInfo>,
    clock_shared: bool,
}

impl Session {
    pub fn new(config: EngineConfig, transfer: TransferConfig) -> Self {
        Session {
            analysis: Arc::new(Mutex::new(Analysis {
                engine: Engine::new(config, 48000.0),
                transfer: TransferFunction::new(transfer, 48000.0),
                reference: ReferenceSource::Off,
                log: None,
                dropped_frames: 0,
            })),
            capture: None,
            output: None,
            generator_tx: None,
            generator_config: GeneratorConfig::default(),
            generator_channel: 0,
            reference_underruns: Arc::new(AtomicU64::new(0)),
            stop: Arc::new(AtomicBool::new(false)),
            worker: None,
            stream: None,
            output_info: None,
            clock_shared: false,
        }
    }

    pub fn analysis(&self) -> Arc<Mutex<Analysis>> {
        Arc::clone(&self.analysis)
    }

    pub fn status(&self) -> SessionStatus {
        let (dropped, errors) = match &self.capture {
            Some(c) => (
                c.stats.dropped_frames.load(Ordering::Relaxed),
                c.stats.stream_errors.load(Ordering::Relaxed),
            ),
            None => (0, 0),
        };
        let reference = self
            .analysis
            .lock()
            .map(|a| a.reference)
            .unwrap_or(ReferenceSource::Off);

        SessionStatus {
            running: self.capture.is_some(),
            stream: self.stream.clone(),
            output: self.output_info.clone(),
            dropped_frames: dropped,
            stream_errors: errors,
            reference_underruns: self.reference_underruns.load(Ordering::Relaxed),
            reference,
            clock_shared: self.clock_shared,
        }
    }

    /// Change the generator settings.
    ///
    /// Sent to the audio thread over a channel rather than written through a
    /// lock: the output callback cannot block, and a `try_lock` that
    /// occasionally fails would drop the change silently.
    pub fn set_generator(&mut self, config: GeneratorConfig, channel: usize) {
        self.generator_config = config;
        self.generator_channel = channel;
        if let Some(tx) = &self.generator_tx {
            let _ = tx.send(GeneratorCommand { config, channel });
        }
    }

    pub fn set_reference(&mut self, reference: ReferenceSource) -> Result<(), String> {
        let mut a = self.analysis.lock().map_err(poisoned)?;
        if a.reference != reference {
            a.reference = reference;
            a.transfer.reset();
        }
        Ok(())
    }

    /// Open an input, and an output if the generator needs one.
    pub fn start(
        &mut self,
        app: AppHandle,
        options: CaptureOptions,
        generator_channel: usize,
    ) -> Result<StreamInfo, String> {
        self.stop();
        self.generator_channel = generator_channel;
        self.reference_underruns.store(0, Ordering::Relaxed);

        let (capture, mut consumer) = leqtion_audio::open(options).map_err(|e| e.to_string())?;
        let info = capture.info.clone();

        // The device decides the sample rate, so every filter coefficient, band
        // edge and transform length is rebuilt around what we actually got.
        {
            let mut a = self.analysis.lock().map_err(poisoned)?;
            let config = a.engine.config().clone();
            a.engine.reconfigure(config, info.sample_rate as f64);
            a.engine.reset_measurement();
            let tf = a.transfer.config();
            a.transfer.reconfigure(tf, info.sample_rate as f64);
        }

        // The output opens on the same device and rate as the input, so the two
        // share a clock. See `OutputOptions`.
        let (generator_tx, generator_rx) = std::sync::mpsc::channel::<GeneratorCommand>();
        let tap_capacity = info.sample_rate as usize * 2;
        let (mut tap_tx, mut tap_rx) = rtrb::RingBuffer::<f32>::new(tap_capacity);

        // Pick the output device before opening, rather than opening and
        // retrying: the fill closure owns the generator and the tap producer,
        // neither of which can be cloned, so it can only be handed over once.
        //
        // On an interface — a Scarlett, a Dante card — input and output are one
        // device and the name matches. On a laptop they are two ("MacBook Pro
        // Microphone" and "MacBook Pro Speakers"), and insisting on one device
        // would silently disable the generator on the commonest machine anyone
        // will try this on. The fallback is not free and is not hidden: two
        // devices means two clocks, so the internal reference drifts and the
        // delay has to be found again every few minutes. `clock_shared` carries
        // that to the UI.
        let same_device_output = leqtion_audio::output_devices(Some(&info.host))
            .map(|outs| outs.iter().any(|d| d.name == info.device))
            .unwrap_or(false);
        let output_device = if same_device_output {
            Some(info.device.clone())
        } else {
            tracing::info!(
                "{} has no output side; the generator will use the default output, on a separate clock",
                info.device
            );
            None
        };

        let mut generator = Generator::new(self.generator_config, info.sample_rate as f64);
        let mut channel = generator_channel;
        let output = leqtion_audio::open_output(
            OutputOptions {
                host: Some(info.host.clone()),
                device: output_device,
                sample_rate: Some(info.sample_rate),
                buffer_frames: None,
            },
            move |buf, channels| {
                while let Ok(c) = generator_rx.try_recv() {
                    generator.apply(c.config);
                    channel = c.channel;
                }
                generator.fill_interleaved(buf, channels, channel);

                // Publish the mono signal that was actually generated, for the
                // internal reference tap. If the analysis thread is not keeping
                // up the surplus is dropped here rather than blocking the
                // driver; the underrun counter on the other side is what makes
                // that visible.
                let target = channel.min(channels.saturating_sub(1));
                let frames = buf.len() / channels.max(1);
                for f in 0..frames {
                    let _ = tap_tx.push(buf[f * channels + target]);
                }
            },
        );

        let output_info = match output {
            Ok(o) => {
                let i = o.info.clone();
                self.clock_shared = i.device == info.device;
                self.output = Some(o);
                Some(i)
            }
            Err(e) => {
                // A missing output is not fatal — the meter still works, and a
                // measurement microphone with no generator is a normal way to
                // use this. It just means no internal reference.
                tracing::warn!("no output available, generator disabled: {e}");
                self.clock_shared = false;
                None
            }
        };

        let analysis = Arc::clone(&self.analysis);
        let underruns = Arc::clone(&self.reference_underruns);
        // The log records dropped frames per row, and the count lives on the
        // capture's stats. Cloned into the worker so a row can carry it without
        // the analysis thread reaching back for the session lock it is holding
        // no part of.
        let capture_stats = Arc::clone(&capture.stats);
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let channels = info.channels as usize;

        let worker = std::thread::Builder::new()
            .name("leqtion-analysis".into())
            .spawn(move || {
                let mut buf = vec![0.0f32; CHUNK];
                let mut measurement: Vec<f32> = Vec::with_capacity(CHUNK);
                let mut reference: Vec<f32> = Vec::with_capacity(CHUNK);
                let mut last_frame = Instant::now();

                while !thread_stop.load(Ordering::Relaxed) {
                    let available = consumer.slots();
                    if available == 0 {
                        std::thread::sleep(IDLE_SLEEP);
                    } else {
                        // Only whole frames, so a partial frame at the end of a
                        // chunk never shifts the channel interleaving by one and
                        // silently swaps the reference for the measurement.
                        let want = available.min(CHUNK) / channels * channels;
                        if want == 0 {
                            std::thread::sleep(IDLE_SLEEP);
                        } else if let Ok(chunk) = consumer.read_chunk(want) {
                            let (first, second) = chunk.as_slices();
                            buf[..first.len()].copy_from_slice(first);
                            buf[first.len()..first.len() + second.len()].copy_from_slice(second);
                            let n = first.len() + second.len();
                            chunk.commit_all();
                            let frames = n / channels;

                            if let Ok(mut a) = analysis.lock() {
                                a.dropped_frames =
                                    capture_stats.dropped_frames.load(Ordering::Relaxed);
                                let interval_done =
                                    a.engine.push_interleaved(&buf[..n], channels);

                                // One row per completed interval, taken from the
                                // engine's own history. A write failure stops the
                                // log and leaves the measurement running: losing a
                                // file is bad, losing the measurement because a
                                // disk filled up would be worse.
                                if interval_done && a.log.is_some() {
                                    let latest = a.engine.history_latest();
                                    let calibrated = a.engine.frame().calibrated;
                                    let dropped = a.dropped_frames;
                                    if let Some(log) = a.log.as_mut() {
                                        if let Err(e) = log.write(&latest, calibrated, dropped) {
                                            tracing::error!("logging stopped: {e}");
                                            a.log = None;
                                        }
                                    }
                                }

                                match a.reference {
                                    ReferenceSource::Off => {
                                        // Still drain the tap, or it fills up and
                                        // the first transfer measurement after
                                        // switching it on starts from stale audio.
                                        while tap_rx.pop().is_ok() {}
                                    }
                                    ReferenceSource::Internal => {
                                        reference.clear();
                                        let mut missing = 0u64;
                                        for _ in 0..frames {
                                            match tap_rx.pop() {
                                                Ok(v) => reference.push(v),
                                                Err(_) => {
                                                    reference.push(0.0);
                                                    missing += 1;
                                                }
                                            }
                                        }
                                        if missing > 0 {
                                            underruns.fetch_add(missing, Ordering::Relaxed);
                                        }
                                        let select = a.engine.config().channel;
                                        fold_channels(
                                            &buf[..n],
                                            channels,
                                            select,
                                            &mut measurement,
                                        );
                                        a.transfer.push_pairs(&reference, &measurement);
                                    }
                                    ReferenceSource::Loopback { channel: c } => {
                                        while tap_rx.pop().is_ok() {}
                                        fold_channels(
                                            &buf[..n],
                                            channels,
                                            leqtion_dsp::engine::ChannelSelect::Channel {
                                                index: c,
                                            },
                                            &mut reference,
                                        );
                                        let select = a.engine.config().channel;
                                        fold_channels(
                                            &buf[..n],
                                            channels,
                                            select,
                                            &mut measurement,
                                        );
                                        a.transfer.push_pairs(&reference, &measurement);
                                    }
                                }
                            }
                        }
                    }

                    if last_frame.elapsed() >= FRAME_INTERVAL {
                        last_frame = Instant::now();
                        let payload = analysis.lock().ok().map(|a| FramePayload {
                            meter: a.engine.frame(),
                            transfer: match a.reference {
                                ReferenceSource::Off => None,
                                _ => Some(a.transfer.reading()),
                            },
                        });
                        if let Some(payload) = payload {
                            // A send failure means the window has gone. The
                            // measurement carries on regardless.
                            let _ = app.emit(FRAME_EVENT, payload);
                        }
                    }
                }
            })
            .map_err(|e| e.to_string())?;

        self.capture = Some(capture);
        self.generator_tx = Some(generator_tx);
        self.stop = stop;
        self.worker = Some(worker);
        self.stream = Some(info.clone());
        self.output_info = output_info;
        Ok(info)
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(w) = self.worker.take() {
            let _ = w.join();
        }
        if let Some(o) = self.output.take() {
            o.stop();
        }
        if let Some(c) = self.capture.take() {
            c.stop();
        }
        self.generator_tx = None;
        self.stream = None;
        self.output_info = None;
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.stop();
    }
}

/// A poisoned lock means the analysis thread panicked mid-measurement. There is
/// no sensible recovery — the engines' internal state is unknown — so this
/// reports rather than papers over it.
fn poisoned<T>(_: std::sync::PoisonError<T>) -> String {
    "the analysis thread failed; stop and restart the measurement".to_string()
}
