//! The running measurement: an open input, an analysis thread, and one engine.
//!
//! ```text
//!   cpal callback ──▶ ring ──▶ analysis thread ──▶ Engine ──▶ frame event ──▶ UI
//!                                    (this file)
//! ```
//!
//! The analysis thread is the only thing that calls `Engine::push`, and it does
//! so on every sample it can get. Frames are emitted to the UI on a timer
//! *inside* the same loop rather than from a separate ticker, which means a UI
//! that stops listening can never make the engine skip audio: integration is
//! driven by samples, display by the clock, and the two do not interact.
//!
//! Everything the UI can ask for goes through `Mutex<Engine>`. That lock is only
//! ever taken off the audio callback — see `leqtion-audio` for why that matters.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use leqtion_audio::{Capture, CaptureOptions, StreamInfo};
use leqtion_dsp::engine::{Engine, EngineConfig, Frame};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

/// Event name the UI listens on for frames.
pub const FRAME_EVENT: &str = "leqtion://frame";

/// Display rate. Fast enough that a meter looks continuous, slow enough that
/// serialising a few hundred band levels costs nothing worth measuring.
const FRAME_INTERVAL: Duration = Duration::from_millis(33);

/// How long the analysis thread sleeps when the ring is empty.
///
/// Short enough not to let the ring build up a visible lag, long enough not to
/// spin a core. The ring holds two seconds, so this is nowhere near tight.
const IDLE_SLEEP: Duration = Duration::from_millis(2);

/// Samples pulled from the ring per iteration.
///
/// Sized so a 65536-point transform at its shortest hop still gets whole hops
/// delivered promptly, without the engine being called back thousands of times
/// a second with a handful of samples each.
const CHUNK: usize = 4096;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionStatus {
    pub running: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<StreamInfo>,
    /// Frames the audio callback had to throw away because the analysis thread
    /// fell behind. Non-zero means time is missing and every LEQ on screen is
    /// suspect — the UI says so rather than hiding it.
    pub dropped_frames: u64,
    pub stream_errors: u64,
}

pub struct Session {
    engine: Arc<Mutex<Engine>>,
    capture: Option<Capture>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    stream: Option<StreamInfo>,
}

impl Session {
    /// A session with no input open. The engine still exists, so the UI has
    /// something coherent to render and configure before a device is chosen.
    pub fn new(config: EngineConfig) -> Self {
        Session {
            engine: Arc::new(Mutex::new(Engine::new(config, 48000.0))),
            capture: None,
            stop: Arc::new(AtomicBool::new(false)),
            worker: None,
            stream: None,
        }
    }

    pub fn engine(&self) -> Arc<Mutex<Engine>> {
        Arc::clone(&self.engine)
    }

    pub fn status(&self) -> SessionStatus {
        let (dropped, errors) = match &self.capture {
            Some(c) => (
                c.stats.dropped_frames.load(Ordering::Relaxed),
                c.stats.stream_errors.load(Ordering::Relaxed),
            ),
            None => (0, 0),
        };
        SessionStatus {
            running: self.capture.is_some(),
            stream: self.stream.clone(),
            dropped_frames: dropped,
            stream_errors: errors,
        }
    }

    /// Open an input and start analysing.
    ///
    /// Any input already open is closed first. Opening two devices at once is
    /// never what someone means, and half-closing the old one on failure of the
    /// new is worse than starting from a known state.
    pub fn start(
        &mut self,
        app: AppHandle,
        options: CaptureOptions,
    ) -> Result<StreamInfo, String> {
        self.stop();

        let (capture, mut consumer) =
            leqtion_audio::open(options).map_err(|e| e.to_string())?;
        let info = capture.info.clone();

        // The device decides the sample rate, so every filter coefficient and
        // band edge is rebuilt around what we actually got — not what was asked
        // for.
        {
            let mut engine = self.engine.lock().map_err(poisoned)?;
            let config = engine.config().clone();
            engine.reconfigure(config, info.sample_rate as f64);
            engine.reset_measurement();
        }

        let engine = Arc::clone(&self.engine);
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let channels = info.channels as usize;

        let worker = std::thread::Builder::new()
            .name("leqtion-analysis".into())
            .spawn(move || {
                let mut buf = vec![0.0f32; CHUNK];
                let mut last_frame = Instant::now();

                while !thread_stop.load(Ordering::Relaxed) {
                    let available = consumer.slots();
                    if available == 0 {
                        std::thread::sleep(IDLE_SLEEP);
                    } else {
                        // Only whole frames, so a partial frame at the end of a
                        // chunk never shifts the channel interleaving by one and
                        // silently swaps left for right for the rest of the run.
                        let want = available.min(CHUNK) / channels * channels;
                        if want == 0 {
                            std::thread::sleep(IDLE_SLEEP);
                        } else if let Ok(chunk) = consumer.read_chunk(want) {
                            let (first, second) = chunk.as_slices();
                            buf[..first.len()].copy_from_slice(first);
                            buf[first.len()..first.len() + second.len()].copy_from_slice(second);
                            let n = first.len() + second.len();
                            chunk.commit_all();

                            if let Ok(mut e) = engine.lock() {
                                e.push_interleaved(&buf[..n], channels);
                            }
                        }
                    }

                    if last_frame.elapsed() >= FRAME_INTERVAL {
                        last_frame = Instant::now();
                        let frame: Option<Frame> = engine.lock().ok().map(|e| e.frame());
                        if let Some(frame) = frame {
                            // A send failure means the window has gone. Nothing
                            // to do about it here, and the measurement carries on.
                            let _ = app.emit(FRAME_EVENT, frame);
                        }
                    }
                }
            })
            .map_err(|e| e.to_string())?;

        self.capture = Some(capture);
        self.stop = stop;
        self.worker = Some(worker);
        self.stream = Some(info.clone());
        Ok(info)
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(w) = self.worker.take() {
            let _ = w.join();
        }
        if let Some(c) = self.capture.take() {
            c.stop();
        }
        self.stream = None;
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.stop();
    }
}

/// A poisoned engine lock means the analysis thread panicked mid-measurement.
/// There is no sensible recovery — the engine's internal state is unknown — so
/// this reports rather than papers over it.
fn poisoned<T>(_: std::sync::PoisonError<T>) -> String {
    "the analysis thread failed; stop and restart the measurement".to_string()
}
