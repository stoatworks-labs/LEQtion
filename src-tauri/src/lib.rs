//! LEQtion — the Tauri layer.
//!
//! Thin on purpose. Everything that decides a number lives in `leqtion-dsp`,
//! everything that touches a driver lives in `leqtion-audio`, and this crate
//! only wires them to a window: enumerate inputs, start and stop a session,
//! forward configuration, persist settings.
//!
//! Commands never do analysis and never block on audio. If a command here grows
//! a loop over samples, it is in the wrong crate.

mod logger;
mod project;
mod session;
mod settings;

use std::sync::{Arc, Mutex};

use leqtion_audio::{CaptureOptions, DeviceInfo, HostInfo};
use leqtion_dsp::bands::BandPlan;
use leqtion_dsp::calibration::{Calibration, CalibrationStatus, CalibrationTarget, STANDARD_TARGETS};
use leqtion_dsp::engine::{EngineConfig, Frame};
use leqtion_dsp::generator::GeneratorConfig;
use leqtion_dsp::history::{HistoryPoint, SeriesInfo};
use leqtion_dsp::transfer::{DelayEstimate, TransferConfig, TransferPlan};
use serde::Serialize;
use logger::{default_filename, LogStatus, Logger};
use project::{ProjectStore, ProjectSummary, Show, ShowSummary};
use session::{Analysis, ReferenceSource, Session, SessionStatus};
use settings::Settings;
use tauri::{AppHandle, Manager, State};

struct AppState {
    session: Mutex<Session>,
    settings: Mutex<Settings>,
    settings_path: std::path::PathBuf,
    projects: ProjectStore,
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
    /// Every project under the projects root, and the one that was open last if it
    /// is still there. A project that has since been moved or deleted resolves to
    /// `None` and the app opens without one, which is a normal state.
    projects: Vec<ProjectSummary>,
    project: Option<ProjectSummary>,
    shows: Vec<ShowSummary>,
    projects_root: String,
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

    // A missing or unreadable project is not an error at startup. The pointer in
    // settings is a convenience, and a folder that has been moved since must not stop
    // the app opening — `docs/tuning.md` §1.3.
    let project = settings
        .last_project
        .as_deref()
        .and_then(|dir| state.projects.summary(dir).ok());
    let shows = project
        .as_ref()
        .and_then(|p| state.projects.shows(&p.dir).ok())
        .unwrap_or_default();

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
        projects: state.projects.list(),
        project,
        shows,
        projects_root: state.projects.root().to_string_lossy().into_owned(),
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
        // A calibrator run always wins. It measured this capsule on this
        // input; a platform profile only describes the class of input the
        // samples arrived through, and would be the worse of the two answers
        // in every case where they disagree.
        let cal = settings.calibration_for(&info.device).cloned().or_else(|| {
            leqtion_audio::profiles::profile_for(leqtion_audio::profiles::current_input_path())
                .map(|p| p.calibration(&info.device))
        });
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


/// Every series the history is recording, so the chart can offer them by name.
#[tauri::command]
fn history_series(state: State<'_, AppState>) -> Result<Vec<SeriesInfo>, String> {
    state.with_analysis(|a| a.engine.history_series())
}

/// One series, over the last `seconds`, bucketed to at most `max_points`.
///
/// Bucketed in the engine rather than the browser: thinning a level trace by
/// dropping points is how a chart quietly loses every peak it was drawn to
/// show, and that decision belongs where the numbers are.
#[tauri::command]
fn history_view(
    state: State<'_, AppState>,
    id: String,
    seconds: f64,
    max_points: usize,
) -> Result<Vec<HistoryPoint>, String> {
    state.with_analysis(|a| a.engine.history_view(&id, seconds, max_points))
}

/// Start writing the measurement to a CSV.
///
/// `path` is optional; without one the log goes to a dated file in the app's own
/// log directory, so "start logging" always works and never opens a dialog
/// someone has to answer before the thing they wanted to capture has happened.
#[tauri::command]
fn start_logging(
    app: AppHandle,
    state: State<'_, AppState>,
    path: Option<String>,
) -> Result<LogStatus, String> {
    let started_at = now_rfc3339();
    let path = match path {
        Some(p) if !p.trim().is_empty() => std::path::PathBuf::from(p),
        _ => app
            .path()
            .app_log_dir()
            .map_err(|e| format!("no log directory: {e}"))?
            .join(default_filename(&started_at)),
    };

    let (device, sample_rate) = {
        let session = state
            .session
            .lock()
            .map_err(|_| "the session lock is poisoned; restart LEQtion")?;
        let status = session.status();
        (
            status.stream.as_ref().map(|s| s.device.clone()),
            status.stream.as_ref().map(|s| s.sample_rate as f64),
        )
    };

    let version = env!("CARGO_PKG_VERSION").to_string();
    let logger = state.with_analysis(|a| {
        let series = a.engine.history_series();
        let interval = a.engine.history_config().interval_seconds;
        let calibrated = a.engine.frame().calibrated;
        Logger::create(
            path,
            &series,
            logger::LogMeta {
                interval_seconds: interval,
                calibrated,
                device: device.as_deref(),
                sample_rate: sample_rate.unwrap_or_else(|| a.engine.sample_rate()),
                version: &version,
                started_at,
            },
        )
    })??;

    state.with_analysis(|a| {
        a.log = Some(logger);
        a.log.as_ref().map(|l| l.status()).unwrap_or_default()
    })
}

#[tauri::command]
fn stop_logging(state: State<'_, AppState>) -> Result<LogStatus, String> {
    state.with_analysis(|a| {
        a.log = None;
        LogStatus {
            interval_seconds: a.engine.history_config().interval_seconds,
            ..LogStatus::default()
        }
    })
}

#[tauri::command]
fn logging_status(state: State<'_, AppState>) -> Result<LogStatus, String> {
    state.with_analysis(|a| match a.log.as_ref() {
        Some(l) => l.status(),
        None => LogStatus {
            interval_seconds: a.engine.history_config().interval_seconds,
            ..LogStatus::default()
        },
    })
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

// ---------------------------------------------------------------------------
// Projects and shows
//
// A project is a folder grouping shows; a show is a complete configuration. See
// `project.rs` and `docs/tuning.md` §1.
//
// Nothing here is required to measure. The app opens, meters and logs with no
// project at all — these commands exist for people who want to keep the work.
// ---------------------------------------------------------------------------

impl AppState {
    fn settings_snapshot(&self) -> Result<Settings, String> {
        Ok(self
            .settings
            .lock()
            .map_err(|_| "settings lock is poisoned")?
            .clone())
    }

    /// Remember where the user is, so the next launch comes back to it.
    fn remember_place(&self, project: Option<&str>, show: Option<&str>) -> Result<(), String> {
        {
            let mut settings = self
                .settings
                .lock()
                .map_err(|_| "settings lock is poisoned")?;
            settings.last_project = project.map(str::to_string);
            settings.last_show = show.map(str::to_string);
        }
        self.save_settings()
    }
}

#[tauri::command]
fn list_projects(state: State<'_, AppState>) -> Vec<ProjectSummary> {
    state.projects.list()
}

#[tauri::command]
fn create_project(state: State<'_, AppState>, name: String) -> Result<ProjectSummary, String> {
    let project = state.projects.create(&name)?;
    state.remember_place(Some(&project.dir), None)?;
    state.projects.summary(&project.dir)
}

/// Open a project: its metadata and the shows inside it. Does **not** load a show —
/// opening a project must not change what is being measured.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct OpenProject {
    project: ProjectSummary,
    shows: Vec<ShowSummary>,
}

#[tauri::command]
fn open_project(state: State<'_, AppState>, dir: String) -> Result<OpenProject, String> {
    let project = state.projects.summary(&dir)?;
    let shows = state.projects.shows(&dir)?;
    state.remember_place(Some(&project.dir), None)?;
    Ok(OpenProject { project, shows })
}

#[tauri::command]
fn close_project(state: State<'_, AppState>) -> Result<(), String> {
    state.remember_place(None, None)
}

#[tauri::command]
fn rename_project(
    state: State<'_, AppState>,
    dir: String,
    name: String,
) -> Result<ProjectSummary, String> {
    let project = state.projects.rename(&dir, &name)?;
    let was_open = state
        .settings
        .lock()
        .map_err(|_| "settings lock is poisoned")?
        .last_project
        .as_deref()
        == Some(dir.as_str());
    if was_open {
        // The directory moved, so a stored pointer to the old one is now dangling.
        let show = state
            .settings
            .lock()
            .map_err(|_| "settings lock is poisoned")?
            .last_show
            .clone();
        state.remember_place(Some(&project.dir), show.as_deref())?;
    }
    state.projects.summary(&project.dir)
}

/// Move a project to the projects root's `.deleted/` folder and return where it went.
///
/// Deliberately not an unlink — see `ProjectStore::delete`. The returned path is shown
/// to the user, because "deleted" that actually means "moved" has to say so.
#[tauri::command]
fn delete_project(state: State<'_, AppState>, dir: String) -> Result<String, String> {
    let moved = state.projects.delete(&dir)?;
    let open = state
        .settings
        .lock()
        .map_err(|_| "settings lock is poisoned")?
        .last_project
        .clone();
    if open.as_deref() == Some(dir.as_str()) {
        state.remember_place(None, None)?;
    }
    Ok(moved.to_string_lossy().into_owned())
}

#[tauri::command]
fn list_shows(state: State<'_, AppState>, dir: String) -> Result<Vec<ShowSummary>, String> {
    state.projects.shows(&dir)
}

/// Save the current configuration as a new show.
#[tauri::command]
fn save_show(
    state: State<'_, AppState>,
    dir: String,
    name: String,
) -> Result<ShowSummary, String> {
    if name.trim().is_empty() {
        return Err("a show needs a name".into());
    }
    let settings = state.settings_snapshot()?;
    let calibration = state.with_analysis(|a| a.engine.calibration().cloned())?;
    let show = Show::capture(&name, &settings, calibration);
    state.projects.save_show(&dir, &show)?;
    state.remember_place(Some(&dir), Some(&show.id))?;
    Ok(ShowSummary::from(&show))
}

/// Overwrite an existing show with the current configuration, keeping its name and id.
#[tauri::command]
fn update_show(state: State<'_, AppState>, dir: String, id: String) -> Result<ShowSummary, String> {
    let mut show = state.projects.load_show(&dir, &id)?;
    let settings = state.settings_snapshot()?;
    let calibration = state.with_analysis(|a| a.engine.calibration().cloned())?;
    show.update_from(&settings, calibration);
    state.projects.save_show(&dir, &show)?;
    state.remember_place(Some(&dir), Some(&show.id))?;
    Ok(ShowSummary::from(&show))
}

#[tauri::command]
fn rename_show(
    state: State<'_, AppState>,
    dir: String,
    id: String,
    name: String,
    notes: Option<String>,
) -> Result<ShowSummary, String> {
    if name.trim().is_empty() {
        return Err("a show needs a name".into());
    }
    let mut show = state.projects.load_show(&dir, &id)?;
    show.name = name.trim().to_string();
    if let Some(notes) = notes {
        show.notes = notes;
    }
    show.modified = now_rfc3339();
    state.projects.save_show(&dir, &show)?;
    Ok(ShowSummary::from(&show))
}

#[tauri::command]
fn delete_show(state: State<'_, AppState>, dir: String, id: String) -> Result<String, String> {
    let moved = state.projects.delete_show(&dir, &id)?;
    let open = state
        .settings
        .lock()
        .map_err(|_| "settings lock is poisoned")?
        .last_show
        .clone();
    if open.as_deref() == Some(id.as_str()) {
        state.remember_place(Some(&dir), None)?;
    }
    Ok(moved.to_string_lossy().into_owned())
}

/// Everything the UI needs after a show is applied, in one call — same reasoning as
/// `startup`: the whole window changes at once, so it should change from one answer.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ShowApplied {
    show: ShowSummary,
    settings: Settings,
    plan: BandPlan,
    transfer_plan: TransferPlan,
    status: SessionStatus,
}

/// Load a show and apply it.
///
/// Applies the engine, transfer, generator and reference configuration and hands the
/// layout back for the frontend to render. Two things it deliberately does **not** do:
///
/// - **It does not open the audio device.** The show remembers which input it used and
///   that is offered as the selection, but a show that loads is not a show that starts
///   measuring. Someone loading a show to look at its settings has not asked to open a
///   stream, and on a shared interface that is not a free action.
/// - **It does not touch the calibration.** The engine keeps whatever belongs to the
///   device that is actually open. See `docs/tuning.md` §1.1 — the show's snapshot
///   describes what it *was* measured with, and `Show::restore` cannot return it.
#[tauri::command]
fn load_show(state: State<'_, AppState>, dir: String, id: String) -> Result<ShowApplied, String> {
    let show = state.projects.load_show(&dir, &id)?;
    let restore = show.restore();

    let (plan, transfer_plan) = state.with_analysis(|a| {
        let rate = a.engine.sample_rate();
        a.engine.reconfigure(restore.engine.clone(), rate);
        let tf_rate = a.transfer.sample_rate();
        a.transfer.reconfigure(restore.transfer, tf_rate);
        (a.engine.plan().clone(), a.transfer.plan().clone())
    })?;

    {
        let mut session = state
            .session
            .lock()
            .map_err(|_| "the session lock is poisoned; restart LEQtion")?;
        // `restore.generator` is guaranteed silent — `Show::restore` forces the signal
        // Off. Sending it here is what makes a loaded show stop a generator that was
        // already running, rather than leaving the previous show's noise in the PA.
        session.set_generator(restore.generator, restore.generator_channel);
        session.set_reference(restore.reference)?;
    }

    {
        let mut settings = state
            .settings
            .lock()
            .map_err(|_| "settings lock is poisoned")?;
        settings.engine = restore.engine;
        settings.transfer = restore.transfer;
        settings.generator = restore.generator;
        settings.generator_channel = restore.generator_channel;
        settings.reference = restore.reference;
        settings.host = restore.host;
        settings.device = restore.device;
        settings.sample_rate = restore.sample_rate;
        settings.layout = restore.layout;
        settings.last_project = Some(dir.clone());
        settings.last_show = Some(show.id.clone());
    }
    state.save_settings()?;

    let status = state
        .session
        .lock()
        .map_err(|_| "the session lock is poisoned; restart LEQtion")?
        .status();

    Ok(ShowApplied {
        show: ShowSummary::from(&show),
        settings: state.settings_snapshot()?,
        plan,
        transfer_plan,
        status,
    })
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

            // Projects live in the user's documents, not in the app's config
            // directory: they are work someone will want to find, back up and hand
            // to a colleague, and a folder buried in Application Support is none of
            // those things. The app data directory is the fallback for a platform
            // that will not give us Documents.
            let projects_root = app
                .path()
                .document_dir()
                .or_else(|_| app.path().app_data_dir())
                .unwrap_or_else(|_| dir.clone())
                .join("LEQtion");

            app.manage(AppState {
                session: Mutex::new(Session::new(settings.engine.clone(), settings.transfer)),
                settings: Mutex::new(settings),
                settings_path,
                projects: ProjectStore::new(projects_root),
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
            history_series,
            history_view,
            start_logging,
            stop_logging,
            logging_status,
            list_projects,
            create_project,
            open_project,
            close_project,
            rename_project,
            delete_project,
            list_shows,
            save_show,
            update_show,
            load_show,
            rename_show,
            delete_show,
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
