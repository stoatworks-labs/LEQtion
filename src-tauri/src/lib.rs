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
use leqtion_dsp::engine::{Engine, EngineConfig, Frame};
use serde::Serialize;
use session::{Session, SessionStatus};
use settings::Settings;
use tauri::{AppHandle, Manager, State};

struct AppState {
    session: Mutex<Session>,
    settings: Mutex<Settings>,
    settings_path: std::path::PathBuf,
}

impl AppState {
    fn engine(&self) -> Result<Arc<Mutex<Engine>>, String> {
        Ok(self
            .session
            .lock()
            .map_err(|_| "the session lock is poisoned; restart LEQtion")?
            .engine())
    }

    /// Run something against the engine.
    ///
    /// Every command that reads or writes measurement state goes through here,
    /// so the lock is taken and released in one place and no command can
    /// accidentally hold it across an emit or a file write.
    fn with_engine<T>(&self, f: impl FnOnce(&mut Engine) -> T) -> Result<T, String> {
        let engine = self.engine()?;
        let mut guard = engine
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
    let plan = state.with_engine(|e| e.plan().clone())?;

    Ok(Startup {
        settings,
        hosts,
        devices,
        status,
        plan,
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
    let info = {
        let mut session = state
            .session
            .lock()
            .map_err(|_| "the session lock is poisoned; restart LEQtion")?;
        session.start(app, options.clone())?
    };

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
        state.with_engine(|e| e.set_calibration(cal))?;
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
    state.with_engine(|e| e.frame())
}

#[tauri::command]
fn band_plan(state: State<'_, AppState>) -> Result<BandPlan, String> {
    state.with_engine(|e| e.plan().clone())
}

#[tauri::command]
fn set_config(state: State<'_, AppState>, config: EngineConfig) -> Result<BandPlan, String> {
    let plan = state.with_engine(|e| {
        let rate = e.sample_rate();
        e.reconfigure(config.clone(), rate);
        e.plan().clone()
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
    state.with_engine(|e| e.reset_measurement())
}

#[tauri::command]
fn reset_peak_hold(state: State<'_, AppState>) -> Result<(), String> {
    state.with_engine(|e| e.reset_peak_hold())
}

#[tauri::command]
fn begin_calibration(state: State<'_, AppState>, target: CalibrationTarget) -> Result<(), String> {
    state.with_engine(|e| e.begin_calibration(target))
}

#[tauri::command]
fn calibration_status(state: State<'_, AppState>) -> Result<Option<CalibrationStatus>, String> {
    state.with_engine(|e| e.calibration_status())
}

#[tauri::command]
fn cancel_calibration(state: State<'_, AppState>) -> Result<(), String> {
    state.with_engine(|e| e.cancel_calibration())
}

/// Accept the running calibration and store it against the open device.
///
/// Fails rather than accepting a doubtful one — the reason is in
/// `calibration_status`, and the UI shows it. A calibration is trusted silently
/// for the rest of the measurement, so this is the last point at which a bad one
/// can be refused.
#[tauri::command]
fn accept_calibration(state: State<'_, AppState>) -> Result<Calibration, String> {
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
        .with_engine(|e| e.accept_calibration())?
        .ok_or("the calibration is not steady enough to accept yet")?;
    cal.device = device;
    cal.taken_at = now_rfc3339();

    state.with_engine(|e| e.set_calibration(Some(cal.clone())))?;
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
    state.with_engine(|e| e.set_calibration(None))?;
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
    state.with_engine(|e| e.calibration().cloned())
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
                session: Mutex::new(Session::new(settings.engine.clone())),
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running LEQtion");
}
