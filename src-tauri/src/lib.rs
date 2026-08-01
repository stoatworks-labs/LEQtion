//! LEQtion — the Tauri layer.
//!
//! Thin on purpose. Everything that decides a number lives in `leqtion-dsp`,
//! everything that touches a driver lives in `leqtion-audio`, and this crate
//! only wires them to a window: enumerate inputs, start and stop a session,
//! forward configuration, persist settings.
//!
//! Commands never do analysis and never block on audio. If a command here grows
//! a loop over samples, it is in the wrong crate.

mod session;
mod settings;

use std::sync::{Arc, Mutex};

use leqtion_audio::{CaptureOptions, DeviceInfo, HostInfo};
use leqtion_dsp::bands::BandPlan;
use leqtion_dsp::calibration::{Calibration, CalibrationStatus, CalibrationTarget, STANDARD_TARGETS};
use leqtion_dsp::engine::{EngineConfig, Frame};
use leqtion_dsp::generator::GeneratorConfig;
use leqtion_dsp::transfer::{DelayEstimate, TransferConfig, TransferPlan};
use serde::Serialize;
use session::{Analysis, ReferenceSource, Session, SessionStatus};
use settings::Settings;
use tauri::{AppHandle, Manager, State};

struct AppState {
    session: Mutex<Session>,
    settings: Mutex<Settings>,
    settings_path: std::path::PathBuf,
}

impl AppState {
    fn analysis(&self) -> Result<Arc<Mutex<Analysis>>, String> {
        Ok(self
            .session
            .lock()
            .map_err(|_| "the session lock is poisoned; restart LEQtion")?
            .analysis())
    }

    /// Run something against the engine.
    ///
    /// Every command that reads or writes measurement state goes through here,
    /// so the lock is taken and released in one place and no command can
    /// accidentally hold it across an emit or a file write.
    fn with_analysis<T>(&self, f: impl FnOnce(&mut Analysis) -> T) -> Result<T, String> {
        let analysis = self.analysis()?;
        let mut guard = analysis
            .lock()
            .map_err(|_| "the analysis thread failed; stop and restart the measurement")?;
        Ok(f(&mut guard))
    }

    fn save_settings(&self) -> Result<(), String> {
        let settings = self
            .settings
            .lock()
            .map_err(|_| "settings lock is poisoned")?;
        settings
            .save(&self.settings_path)
            .map_err(|e| format!("could not save settings: {e}"))
    }
}

/// Everything the UI needs on startup, in one call.
///
/// One round trip rather than five, because the first paint depends on all of
/// it and staggering them makes the window flash through several wrong states.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Startup {
    settings: Settings,
    hosts: Vec<HostInfo>,
    devices: Vec<DeviceInfo>,
    status: SessionStatus,
    plan: BandPlan,
    transfer_plan: TransferPlan,
    outputs: Vec<DeviceInfo>,
    calibration_targets: Vec<CalibrationTarget>,
    version: String,
}

#[tauri::command]
fn startup(state: State<'_, AppState>) -> Result<Startup, String> {
    let settings = state
        .settings
        .lock()
        .map_err(|_| "settings lock is poisoned")?
        .clone();
    let hosts = leqtion_audio::hosts();
    // A machine with no input is a perfectly normal state to start in — an
    // empty list is not an error, and refusing to open the window over it would
    // be absurd.
    let devices = leqtion_audio::devices(settings.host.as_deref()).unwrap_or_default();
    let status = state
        .session
        .lock()
        .map_err(|_| "the session lock is poisoned; restart LEQtion")?
        .status();
    let (plan, transfer_plan) = state.with_analysis(|a| {
        (a.engine.plan().clone(), a.transfer.plan().clone())
    })?;
    let outputs = leqtion_audio::output_devices(settings.host.as_deref()).unwrap_or_default();

    Ok(Startup {
        settings,
        hosts,
        devices,
        status,
        plan,
        transfer_plan,
        outputs,
        calibration_targets: STANDARD_TARGETS.to_vec(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    })
}

#[tauri::command]
fn list_hosts() -> Vec<HostInfo> {
    leqtion_audio::hosts()
}

#[tauri::command]
fn list_devices(host: Option<String>) -> Result<Vec<DeviceInfo>, String> {
    leqtion_audio::devices(host.as_deref()).map_err(|e| e.to_string())
}

#[tauri::command]
fn start(
    app: AppHandle,
    state: State<'_, AppState>,
    options: CaptureOptions,
) -> Result<SessionStatus, String> {
    let (generator, generator_channel, reference) = {
        let settings = state
            .settings
            .lock()
            .map_err(|_| "settings lock is poisoned")?;
        (settings.generator, settings.generator_channel, settings.reference)
    };

    let info = {
        let mut session = state
            .session
            .lock()
            .map_err(|_| "the session lock is poisoned; restart LEQtion")?;
        session.set_generator(generator, generator_channel);
        session.start(app, options.clone(), generator_channel)?
    };
    state.with_analysis(|a| a.reference = reference)?;

    // Apply the stored calibration for whatever device actually opened, and
    // remember the choice for next launch.
    {
        let mut settings = state
            .settings
            .lock()
            .map_err(|_| "settings lock is poisoned")?;
        settings.host = Some(info.host.clone());
        settings.device = Some(info.device.clone());
        settings.sample_rate = Some(info.sample_rate);
        let cal = settings.calibration_for(&info.device).cloned();
        drop(settings);
        state.with_analysis(|a| a.engine.set_calibration(cal))?;
    }
    state.save_settings()?;

    state
        .session
        .lock()
        .map_err(|_| "the session lock is poisoned; restart LEQtion".to_string())
        .map(|s| s.status())
}

#[tauri::command]
fn stop(state: State<'_, AppState>) -> Result<SessionStatus, String> {
    let mut session = state
        .session
        .lock()
        .map_err(|_| "the session lock is poisoned; restart LEQtion")?;
    session.stop();
    Ok(session.status())
}

#[tauri::command]
fn status(state: State<'_, AppState>) -> Result<SessionStatus, String> {
    Ok(state
        .session
        .lock()
        .map_err(|_| "the session lock is poisoned; restart LEQtion")?
        .status())
}

/// A frame on demand, for the first paint before any event has arrived.
#[tauri::command]
fn frame(state: State<'_, AppState>) -> Result<Frame, String> {
    state.with_analysis(|a| a.engine.frame())
}

#[tauri::command]
fn band_plan(state: State<'_, AppState>) -> Result<BandPlan, String> {
    state.with_analysis(|a| a.engine.plan().clone())
}

#[tauri::command]
fn set_config(state: State<'_, AppState>, config: EngineConfig) -> Result<BandPlan, String> {
    let plan = state.with_analysis(|a| {
        let rate = a.engine.sample_rate();
        a.engine.reconfigure(config.clone(), rate);
        a.engine.plan().clone()
    })?;
    {
        let mut settings = state
            .settings
            .lock()
            .map_err(|_| "settings lock is poisoned")?;
        settings.engine = config;
    }
    state.save_settings()?;
    Ok(plan)
}

#[tauri::command]
fn reset_measurement(state: State<'_, AppState>) -> Result<(), String> {
    state.with_analysis(|a| a.engine.reset_measurement())
}

#[tauri::command]
fn reset_peak_hold(state: State<'_, AppState>) -> Result<(), String> {
    state.with_analysis(|a| a.engine.reset_peak_hold())
}

/// True if the open input is the software generator rather than a device.
///
/// Split out as a plain function of the host name so it can be tested without
/// standing up a session.
fn is_synthetic_source(host: Option<&str>) -> bool {
    host.is_some_and(|h| h.eq_ignore_ascii_case(leqtion_audio::synthetic::HOST_ID))
}

/// Calibration needs a microphone, and the generator is not one.
///
/// This matters more than it looks. A generated 1 kHz sine satisfies every gate
/// the calibration workflow has — it is perfectly steady, exactly on frequency,
/// unclipped and far above the noise floor — so it would sail through and
/// produce a full-scale-to-SPL offset invented out of nothing, which every
/// reading afterwards would silently inherit. The engine cannot catch this,
/// because from inside the analysis there is no difference between a calibrator
/// on a capsule and a sine on a wire. Only the source knows.
fn refuse_calibration_without_a_microphone(state: &AppState) -> Result<(), String> {
    let host = state
        .session
        .lock()
        .map_err(|_| "the session lock is poisoned; restart LEQtion")?
        .status()
        .stream
        .map(|s| s.host);

    if is_synthetic_source(host.as_deref()) {
        return Err("The signal generator is not an acoustic reference — there is no \
                    microphone in the chain, so there is nothing to calibrate against. \
                    Open a real input and use a hardware calibrator."
            .into());
    }
    Ok(())
}

#[tauri::command]
fn begin_calibration(state: State<'_, AppState>, target: CalibrationTarget) -> Result<(), String> {
    refuse_calibration_without_a_microphone(&state)?;
    state.with_analysis(|a| a.engine.begin_calibration(target))
}

#[tauri::command]
fn calibration_status(state: State<'_, AppState>) -> Result<Option<CalibrationStatus>, String> {
    state.with_analysis(|a| a.engine.calibration_status())
}

#[tauri::command]
fn cancel_calibration(state: State<'_, AppState>) -> Result<(), String> {
    state.with_analysis(|a| a.engine.cancel_calibration())
}

/// Accept the running calibration and store it against the open device.
///
/// Fails rather than accepting a doubtful one — the reason is in
/// `calibration_status`, and the UI shows it. A calibration is trusted silently
/// for the rest of the measurement, so this is the last point at which a bad one
/// can be refused.
#[tauri::command]
fn accept_calibration(state: State<'_, AppState>) -> Result<Calibration, String> {
    // Also checked on accept, not only on begin: the source can change under a
    // dialog that is already open, and this is the call that writes an offset
    // to disk for every future session on that device name.
    refuse_calibration_without_a_microphone(&state)?;

    let device = {
        let session = state
            .session
            .lock()
            .map_err(|_| "the session lock is poisoned; restart LEQtion")?;
        session
            .status()
            .stream
            .map(|s| s.device)
            .ok_or("no input is open")?
    };

    let mut cal = state
        .with_analysis(|a| a.engine.accept_calibration())?
        .ok_or("the calibration is not steady enough to accept yet")?;
    cal.device = device;
    cal.taken_at = now_rfc3339();

    state.with_analysis(|a| a.engine.set_calibration(Some(cal.clone())))?;
    {
        let mut settings = state
            .settings
            .lock()
            .map_err(|_| "settings lock is poisoned")?;
        settings.set_calibration(cal.clone());
    }
    state.save_settings()?;
    Ok(cal)
}

/// Forget the calibration for the open device, so levels go back to dBFS.
#[tauri::command]
fn clear_calibration(state: State<'_, AppState>) -> Result<(), String> {
    let device = {
        let session = state
            .session
            .lock()
            .map_err(|_| "the session lock is poisoned; restart LEQtion")?;
        session.status().stream.map(|s| s.device)
    };
    state.with_analysis(|a| a.engine.set_calibration(None))?;
    if let Some(device) = device {
        let mut settings = state
            .settings
            .lock()
            .map_err(|_| "settings lock is poisoned")?;
        settings.calibrations.retain(|c| c.device != device);
    }
    state.save_settings()
}

#[tauri::command]
fn current_calibration(state: State<'_, AppState>) -> Result<Option<Calibration>, String> {
    state.with_analysis(|a| a.engine.calibration().cloned())
}

/// Store the tile layout. Opaque JSON — see `settings::Settings::layout`.
#[tauri::command]
fn save_layout(state: State<'_, AppState>, layout: serde_json::Value) -> Result<(), String> {
    {
        let mut settings = state
            .settings
            .lock()
            .map_err(|_| "settings lock is poisoned")?;
        settings.layout = layout;
    }
    state.save_settings()
}


// ---------------------------------------------------------------------------
// Generator and transfer function
// ---------------------------------------------------------------------------

#[tauri::command]
fn list_output_devices(host: Option<String>) -> Result<Vec<DeviceInfo>, String> {
    leqtion_audio::output_devices(host.as_deref()).map_err(|e| e.to_string())
}

/// Change what the generator is producing, and where it goes.
///
/// Takes effect on the next audio callback — a few milliseconds — and the
/// generator ramps rather than jumps, so this is safe to call from a slider
/// being dragged.
#[tauri::command]
fn set_generator(
    state: State<'_, AppState>,
    config: GeneratorConfig,
    channel: usize,
) -> Result<(), String> {
    {
        let mut session = state
            .session
            .lock()
            .map_err(|_| "the session lock is poisoned; restart LEQtion")?;
        session.set_generator(config, channel);
    }
    {
        let mut settings = state
            .settings
            .lock()
            .map_err(|_| "settings lock is poisoned")?;
        settings.generator = config;
        settings.generator_channel = channel;
    }
    state.save_settings()
}

/// Choose where the transfer function's reference comes from.
#[tauri::command]
fn set_reference(
    state: State<'_, AppState>,
    reference: ReferenceSource,
) -> Result<SessionStatus, String> {
    {
        let mut session = state
            .session
            .lock()
            .map_err(|_| "the session lock is poisoned; restart LEQtion")?;
        session.set_reference(reference)?;
    }
    {
        let mut settings = state
            .settings
            .lock()
            .map_err(|_| "settings lock is poisoned")?;
        settings.reference = reference;
    }
    state.save_settings()?;
    state
        .session
        .lock()
        .map_err(|_| "the session lock is poisoned; restart LEQtion".to_string())
        .map(|s| s.status())
}

#[tauri::command]
fn set_transfer_config(
    state: State<'_, AppState>,
    config: TransferConfig,
) -> Result<TransferPlan, String> {
    let plan = state.with_analysis(|a| {
        let rate = a.transfer.sample_rate();
        a.transfer.reconfigure(config, rate);
        a.transfer.plan().clone()
    })?;
    {
        let mut settings = state
            .settings
            .lock()
            .map_err(|_| "settings lock is poisoned")?;
        settings.transfer = config;
    }
    state.save_settings()?;
    Ok(plan)
}

#[tauri::command]
fn transfer_plan(state: State<'_, AppState>) -> Result<TransferPlan, String> {
    state.with_analysis(|a| a.transfer.plan().clone())
}

#[tauri::command]
fn reset_transfer(state: State<'_, AppState>) -> Result<(), String> {
    state.with_analysis(|a| a.transfer.reset())
}

/// Locate the arrival and report it. Does **not** apply it — the UI shows the
/// figure and its confidence first, because a delay found from a reflection or
/// from noise is worse than no delay at all and the number is the only clue.
#[tauri::command]
fn find_delay(state: State<'_, AppState>) -> Result<Option<DelayEstimate>, String> {
    state.with_analysis(|a| a.transfer.find_delay())
}

#[tauri::command]
fn set_delay_samples(state: State<'_, AppState>, samples: u32) -> Result<(), String> {
    state.with_analysis(|a| a.transfer.set_delay_samples(samples as usize))
}

/// The impulse response, for display. Downsampled by peak-picking if it is
/// longer than the caller asked for: a 65536-point response drawn into 800
/// pixels is 80 samples per pixel, and taking every 80th would miss the peak
/// entirely and draw a flat line where the arrival is.
#[tauri::command]
fn impulse_response(state: State<'_, AppState>, max_points: usize) -> Result<Vec<f32>, String> {
    let ir = state.with_analysis(|a| a.transfer.impulse_response())?;
    let max_points = max_points.clamp(64, 8192);
    if ir.len() <= max_points {
        return Ok(ir);
    }
    let stride = ir.len().div_ceil(max_points);
    Ok(ir
        .chunks(stride)
        .map(|c| {
            c.iter()
                .copied()
                .max_by(|x, y| x.abs().partial_cmp(&y.abs()).unwrap())
                .unwrap_or(0.0)
        })
        .collect())
}

fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let dir = app.path().app_config_dir().unwrap_or_else(|_| ".".into());
            let settings_path = dir.join("settings.json");
            let settings = Settings::load(&settings_path);

            // Logging and crash reports go to the platform log directory. Held
            // for the life of the process: dropping the guard stops the file
            // being written, and the first anyone would know is an empty
            // diagnostics bundle after a fault.
            let guard = diag::init(
                diag::Options::new("leqtion", "LEQTION", env!("CARGO_PKG_VERSION"))
                    .with_default_filter("info")
                    .with_config(&settings),
            );
            match guard {
                Ok(g) => {
                    app.manage(g);
                }
                Err(e) => eprintln!("logging is unavailable: {e}"),
            }

            app.manage(AppState {
                session: Mutex::new(Session::new(settings.engine.clone(), settings.transfer)),
                settings: Mutex::new(settings),
                settings_path,
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            startup,
            list_hosts,
            list_devices,
            start,
            stop,
            status,
            frame,
            band_plan,
            set_config,
            reset_measurement,
            reset_peak_hold,
            begin_calibration,
            calibration_status,
            cancel_calibration,
            accept_calibration,
            clear_calibration,
            current_calibration,
            save_layout,
            list_output_devices,
            set_generator,
            set_reference,
            set_transfer_config,
            transfer_plan,
            reset_transfer,
            find_delay,
            set_delay_samples,
            impulse_response,
        ])
        .run(tauri::generate_context!())
        .expect("error while running LEQtion");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_generator_is_recognised_as_not_a_microphone() {
        assert!(is_synthetic_source(Some(
            leqtion_audio::synthetic::HOST_ID
        )));
        assert!(is_synthetic_source(Some("generator")));
    }

    /// The check must not accidentally cover a real input. Refusing to
    /// calibrate a working microphone would be the more damaging failure of the
    /// two, because the user would have no way round it.
    #[test]
    fn a_real_host_is_still_calibratable() {
        assert!(!is_synthetic_source(Some("CoreAudio")));
        assert!(!is_synthetic_source(Some("WASAPI")));
        assert!(!is_synthetic_source(Some("Asio")));
        assert!(!is_synthetic_source(None));
    }
}
