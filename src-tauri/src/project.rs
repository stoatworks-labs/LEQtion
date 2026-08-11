//! Projects and shows — the container the tuning work lives in.
//!
//! A **show** is a complete, independent configuration: engine, transfer, generator,
//! input device, sample rate, LEQ definitions and tile layout. Loading one puts the
//! application back exactly as it was left.
//!
//! A **project** is a folder that groups shows and, later, owns a trace library shared
//! by all of them. It holds almost nothing itself. Two shows in one project may use
//! different interfaces and different band resolutions; nothing is inherited.
//!
//! See `docs/tuning.md` §1 for why the split is this way round.
//!
//! # What a show does not carry
//!
//! A show stores a *snapshot* of the calibration that was in force when it was saved,
//! and that snapshot is never applied to live audio — see [`Show::restore`], which
//! structurally cannot return one. Live calibration always comes from the device-keyed
//! table in `settings.json`, because a calibration belongs to a microphone and a preamp
//! rather than to an application state. Opening last year's show with this year's
//! microphone must not present dB SPL computed from an offset measured on hardware that
//! is no longer in the room.
//!
//! # On disk
//!
//! ```text
//! <root>/<Project Name>/project.json     name, notes, created
//!                       shows/<id>.json  one complete show each
//!                       traces/          the project's trace library (step 2)
//! <root>/.deleted/                       where removals go, see `delete_*`
//! ```
//!
//! There is no index of shows. The directory *is* the index: it is scanned on demand,
//! so a show file copied in by hand appears, and there is no second source of truth to
//! go stale. Listing reads every show file, which is a few kilobytes each.

use std::path::{Path, PathBuf};

use leqtion_dsp::calibration::Calibration;
use leqtion_dsp::engine::EngineConfig;
use leqtion_dsp::generator::{GeneratorConfig, Signal};
use leqtion_dsp::transfer::TransferConfig;
use serde::{Deserialize, Serialize};

use crate::session::ReferenceSource;
use crate::settings::Settings;

/// Metadata for the project itself. Everything else about a project is the files
/// inside its directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: String,
    pub name: String,
    pub created: String,
    pub modified: String,
    #[serde(default)]
    pub notes: String,
    /// Directory name under the projects root. Not serialised — it is where the
    /// file was found, and storing it as well would let the two disagree.
    #[serde(skip)]
    pub dir: String,
}

impl Project {
    pub fn new(name: &str) -> Self {
        let now = now_rfc3339();
        Project {
            id: make_id(name),
            name: name.trim().to_string(),
            created: now.clone(),
            modified: now,
            notes: String::new(),
            dir: String::new(),
        }
    }
}

/// A complete application configuration, saved under a name.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Show {
    pub id: String,
    pub name: String,
    pub created: String,
    pub modified: String,
    #[serde(default)]
    pub notes: String,

    #[serde(default)]
    pub engine: EngineConfig,
    #[serde(default)]
    pub transfer: TransferConfig,
    #[serde(default)]
    pub generator: GeneratorConfig,
    #[serde(default)]
    pub generator_channel: usize,
    #[serde(default)]
    pub reference: ReferenceSource,
    #[serde(default)]
    pub host: Option<String>,
    #[serde(default)]
    pub device: Option<String>,
    #[serde(default)]
    pub sample_rate: Option<u32>,
    /// Tile layout. Opaque to Rust, exactly as in `Settings::layout`.
    #[serde(default)]
    pub layout: serde_json::Value,

    /// The calibration in force when this show was saved.
    ///
    /// **Recorded, never applied.** It exists so a trace captured under this show can
    /// state what it was measured against. [`Show::restore`] has no field for it, which
    /// is the enforcement — there is no path from here to the engine.
    #[serde(default)]
    pub calibration_snapshot: Option<Calibration>,
}

/// What actually gets pushed back into the application when a show is loaded.
///
/// Deliberately *not* the same type as [`Show`]: it has no calibration field, so a show
/// physically cannot restore an SPL offset for hardware that may not be present.
#[derive(Debug, Clone)]
pub struct ShowRestore {
    pub engine: EngineConfig,
    pub transfer: TransferConfig,
    pub generator: GeneratorConfig,
    pub generator_channel: usize,
    pub reference: ReferenceSource,
    pub host: Option<String>,
    pub device: Option<String>,
    pub sample_rate: Option<u32>,
    pub layout: serde_json::Value,
}

impl Show {
    /// Capture the current application state as a new show.
    pub fn capture(name: &str, settings: &Settings, calibration: Option<Calibration>) -> Self {
        let now = now_rfc3339();
        Show {
            id: make_id(name),
            name: name.trim().to_string(),
            created: now.clone(),
            modified: now,
            notes: String::new(),
            engine: settings.engine.clone(),
            transfer: settings.transfer,
            generator: settings.generator,
            generator_channel: settings.generator_channel,
            reference: settings.reference,
            host: settings.host.clone(),
            device: settings.device.clone(),
            sample_rate: settings.sample_rate,
            layout: settings.layout.clone(),
            calibration_snapshot: calibration,
        }
    }

    /// Overwrite this show's configuration from the current application state,
    /// keeping its identity, name and notes.
    pub fn update_from(&mut self, settings: &Settings, calibration: Option<Calibration>) {
        self.engine = settings.engine.clone();
        self.transfer = settings.transfer;
        self.generator = settings.generator;
        self.generator_channel = settings.generator_channel;
        self.reference = settings.reference;
        self.host = settings.host.clone();
        self.device = settings.device.clone();
        self.sample_rate = settings.sample_rate;
        self.layout = settings.layout.clone();
        self.calibration_snapshot = calibration;
        self.modified = now_rfc3339();
    }

    /// The configuration to restore, with the generator silenced.
    ///
    /// The generator's **level and shaping are restored; its signal is not** — it comes
    /// back `Off` exactly as it does on launch (AGENTS.md §4.8). Loading a show is not a
    /// reason to put pink noise into a PA, and it is a worse moment for it than launch:
    /// at launch nothing is running, whereas a show can be loaded with an output stream
    /// already open and a system live.
    pub fn restore(&self) -> ShowRestore {
        ShowRestore {
            engine: self.engine.clone(),
            transfer: self.transfer,
            generator: GeneratorConfig {
                signal: Signal::Off,
                ..self.generator
            },
            generator_channel: self.generator_channel,
            reference: self.reference,
            host: self.host.clone(),
            device: self.device.clone(),
            sample_rate: self.sample_rate,
            layout: self.layout.clone(),
        }
    }
}

/// A show without its configuration, for pickers and lists.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShowSummary {
    pub id: String,
    pub name: String,
    pub created: String,
    pub modified: String,
    pub notes: String,
    pub device: Option<String>,
}

impl From<&Show> for ShowSummary {
    fn from(s: &Show) -> Self {
        ShowSummary {
            id: s.id.clone(),
            name: s.name.clone(),
            created: s.created.clone(),
            modified: s.modified.clone(),
            notes: s.notes.clone(),
            device: s.device.clone(),
        }
    }
}

/// A project without its shows read in.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSummary {
    pub id: String,
    pub name: String,
    pub dir: String,
    pub created: String,
    pub modified: String,
    pub notes: String,
    pub show_count: usize,
}

/// The projects root — one directory holding every project.
pub struct ProjectStore {
    root: PathBuf,
}

const PROJECT_FILE: &str = "project.json";
const SHOWS_DIR: &str = "shows";
const TRACES_DIR: &str = "traces";
const DELETED_DIR: &str = ".deleted";

impl ProjectStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        ProjectStore { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn project_dir(&self, dir: &str) -> Result<PathBuf, String> {
        // `dir` arrives from the frontend, so it is not trusted to be a bare name.
        // A project called `../../etc` must not escape the root.
        let clean = sanitise_name(dir);
        if clean.is_empty() {
            return Err("that is not a valid project name".into());
        }
        Ok(self.root.join(clean))
    }

    /// Every project under the root, newest first.
    ///
    /// A directory without a readable `project.json` is not a project and is skipped
    /// silently: the root is a place people will put other things.
    pub fn list(&self) -> Vec<ProjectSummary> {
        let Ok(entries) = std::fs::read_dir(&self.root) else {
            return Vec::new();
        };
        let mut out: Vec<ProjectSummary> = entries
            .flatten()
            .filter(|e| e.path().is_dir())
            .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
            .filter_map(|e| {
                let dir = e.file_name().to_string_lossy().into_owned();
                let project = self.open(&dir).ok()?;
                Some(ProjectSummary {
                    id: project.id,
                    name: project.name,
                    created: project.created,
                    modified: project.modified,
                    notes: project.notes,
                    show_count: count_shows(&e.path()),
                    dir,
                })
            })
            .collect();
        out.sort_by(|a, b| b.modified.cmp(&a.modified));
        out
    }

    pub fn create(&self, name: &str) -> Result<Project, String> {
        let clean = sanitise_name(name);
        if clean.is_empty() {
            return Err("a project needs a name".into());
        }
        let dir = self.root.join(&clean);
        if dir.exists() {
            return Err(format!("a project called \"{clean}\" already exists"));
        }
        std::fs::create_dir_all(dir.join(SHOWS_DIR))
            .map_err(|e| format!("could not create the project: {e}"))?;
        std::fs::create_dir_all(dir.join(TRACES_DIR))
            .map_err(|e| format!("could not create the project: {e}"))?;

        let mut project = Project::new(name);
        project.dir = clean;
        self.save(&project)?;
        Ok(project)
    }

    /// A project as the UI sees it.
    ///
    /// [`Project::dir`] is not serialised — it is where the file was found, not
    /// something stored in it — so the frontend is given summaries, which carry the
    /// directory explicitly and are the handle every other call takes.
    pub fn summary(&self, dir: &str) -> Result<ProjectSummary, String> {
        let project = self.open(dir)?;
        Ok(ProjectSummary {
            show_count: count_shows(&self.project_dir(&project.dir)?),
            id: project.id,
            name: project.name,
            dir: project.dir,
            created: project.created,
            modified: project.modified,
            notes: project.notes,
        })
    }

    pub fn open(&self, dir: &str) -> Result<Project, String> {
        let path = self.project_dir(dir)?.join(PROJECT_FILE);
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("could not read {}: {e}", path.display()))?;
        let mut project: Project = serde_json::from_str(&text)
            .map_err(|e| format!("{} is not a readable project: {e}", path.display()))?;
        project.dir = sanitise_name(dir);
        Ok(project)
    }

    pub fn save(&self, project: &Project) -> Result<(), String> {
        let dir = self.project_dir(&project.dir)?;
        std::fs::create_dir_all(&dir).map_err(|e| format!("could not save the project: {e}"))?;
        write_atomic(&dir.join(PROJECT_FILE), project)
    }

    /// Rename a project, which moves its directory.
    ///
    /// The directory name is the display name, so that someone looking at the folder in
    /// a file manager sees what they see in the app. That means a rename is a move, and
    /// a move onto an existing name is refused rather than merged.
    pub fn rename(&self, dir: &str, new_name: &str) -> Result<Project, String> {
        let clean = sanitise_name(new_name);
        if clean.is_empty() {
            return Err("a project needs a name".into());
        }
        let from = self.project_dir(dir)?;
        let to = self.root.join(&clean);
        let mut project = self.open(dir)?;

        if from != to {
            if to.exists() {
                return Err(format!("a project called \"{clean}\" already exists"));
            }
            std::fs::rename(&from, &to).map_err(|e| format!("could not rename: {e}"))?;
        }
        project.name = new_name.trim().to_string();
        project.dir = clean;
        project.modified = now_rfc3339();
        self.save(&project)?;
        Ok(project)
    }

    /// Move a project out of the way.
    ///
    /// Not an unlink. A project is someone's work and a mis-click is not a reason to
    /// lose it, so it moves into `.deleted/` under the root with a timestamp and stays
    /// there until a human clears it out. The UI is expected to say so.
    pub fn delete(&self, dir: &str) -> Result<PathBuf, String> {
        let from = self.project_dir(dir)?;
        if !from.exists() {
            return Err("that project is not there".into());
        }
        let graveyard = self.root.join(DELETED_DIR);
        std::fs::create_dir_all(&graveyard).map_err(|e| format!("could not delete: {e}"))?;
        let to = graveyard.join(format!("{}-{}", sanitise_name(dir), stamp()));
        std::fs::rename(&from, &to).map_err(|e| format!("could not delete: {e}"))?;
        Ok(to)
    }

    // -- shows ---------------------------------------------------------------

    fn shows_dir(&self, dir: &str) -> Result<PathBuf, String> {
        Ok(self.project_dir(dir)?.join(SHOWS_DIR))
    }

    /// Every readable show in a project, newest first.
    ///
    /// A show that will not parse is logged and left alone — **never deleted and never
    /// replaced with a default**. Unlike a tile layout, a show is work someone did, so
    /// the file stays where it is and can be recovered by hand.
    pub fn shows(&self, dir: &str) -> Result<Vec<ShowSummary>, String> {
        let path = self.shows_dir(dir)?;
        let Ok(entries) = std::fs::read_dir(&path) else {
            return Ok(Vec::new());
        };
        let mut out: Vec<ShowSummary> = entries
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
            .filter_map(|e| match read_show(&e.path()) {
                Ok(show) => Some(ShowSummary::from(&show)),
                Err(e) => {
                    tracing::warn!("skipping an unreadable show: {e}");
                    None
                }
            })
            .collect();
        out.sort_by(|a, b| b.modified.cmp(&a.modified));
        Ok(out)
    }

    pub fn load_show(&self, dir: &str, id: &str) -> Result<Show, String> {
        read_show(&self.show_path(dir, id)?)
    }

    pub fn save_show(&self, dir: &str, show: &Show) -> Result<(), String> {
        let path = self.show_path(dir, &show.id)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("could not save the show: {e}"))?;
        }
        write_atomic(&path, show)
    }

    /// Move a show into the project's `.deleted/` folder. See [`Self::delete`].
    pub fn delete_show(&self, dir: &str, id: &str) -> Result<PathBuf, String> {
        let from = self.show_path(dir, id)?;
        if !from.exists() {
            return Err("that show is not there".into());
        }
        let graveyard = self.project_dir(dir)?.join(DELETED_DIR);
        std::fs::create_dir_all(&graveyard).map_err(|e| format!("could not delete: {e}"))?;
        let to = graveyard.join(format!("{}-{}.json", sanitise_id(id), stamp()));
        std::fs::rename(&from, &to).map_err(|e| format!("could not delete: {e}"))?;
        Ok(to)
    }

    fn show_path(&self, dir: &str, id: &str) -> Result<PathBuf, String> {
        let id = sanitise_id(id);
        if id.is_empty() {
            return Err("that is not a valid show".into());
        }
        Ok(self.shows_dir(dir)?.join(format!("{id}.json")))
    }
}

fn read_show(path: &Path) -> Result<Show, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("could not read {}: {e}", path.display()))?;
    serde_json::from_str(&text)
        .map_err(|e| format!("{} is not a readable show: {e}", path.display()))
}

fn count_shows(project_dir: &Path) -> usize {
    std::fs::read_dir(project_dir.join(SHOWS_DIR))
        .map(|entries| {
            entries
                .flatten()
                .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
                .count()
        })
        .unwrap_or(0)
}

/// Write JSON through a temporary file and rename, as `Settings::save` does.
///
/// A rename is atomic on every platform this ships to, so an interrupted write leaves
/// the previous file intact rather than a truncated one that parses as nothing.
fn write_atomic<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|e| format!("could not encode: {e}"))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, bytes).map_err(|e| format!("could not write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("could not write {}: {e}", path.display()))
}

/// Make a string safe to use as a single path component, on every platform.
///
/// Windows is the strict one and therefore the one this targets: it forbids
/// `\ / : * ? " < > |`, forbids trailing dots and spaces, and reserves a list of device
/// names that cannot be used even with an extension. Getting this wrong produces a
/// project that saves on macOS and cannot be opened on Windows.
fn sanitise_name(name: &str) -> String {
    const RESERVED: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
        "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];

    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '-',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect();

    // Trailing dots and spaces are silently stripped by Windows, which turns
    // "Project." into "Project" and makes a saved path unfindable. Leading dots go
    // too: a name beginning with one is a hidden directory on Unix, and `list` skips
    // hidden directories, so it would save successfully and then never appear.
    let trimmed = cleaned.trim().trim_matches(['.', ' ']).trim();

    if trimmed.is_empty() || trimmed.chars().all(|c| c == '.') {
        return String::new();
    }
    if RESERVED
        .iter()
        .any(|r| trimmed.eq_ignore_ascii_case(r))
    {
        return format!("{trimmed}_");
    }
    trimmed.chars().take(120).collect()
}

/// Ids are ours, not the user's, so they get the strict treatment: the set that is
/// safe in a filename everywhere, and nothing else.
fn sanitise_id(id: &str) -> String {
    id.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(80)
        .collect()
}

/// A stable, human-readable id: a slug of the name plus a timestamp.
///
/// Human-readable because these are filenames someone may end up looking at, and
/// timestamped because two shows called "FOH" in one project is normal. The id never
/// changes after creation — renaming a show changes its name, not its file.
fn make_id(name: &str) -> String {
    let slug: String = name
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-")
        .chars()
        .take(40)
        .collect();
    let slug = if slug.is_empty() { "show".to_string() } else { slug };
    format!("{slug}-{}", stamp())
}

fn stamp() -> String {
    let now = time::OffsetDateTime::now_utc();
    format!(
        "{:04}{:02}{:02}-{:02}{:02}{:02}",
        now.year(),
        u8::from(now.month()),
        now.day(),
        now.hour(),
        now.minute(),
        now.second()
    )
}

fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use leqtion_dsp::calibration::{Calibration, CalibrationTarget};

    /// Each test gets its own root, because these tests write files and a shared
    /// directory would make them order-dependent.
    fn temp_root(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "leqtion-projects-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn a_calibration() -> Calibration {
        let mut cal = Calibration::new(CalibrationTarget::default(), -26.0);
        cal.device = "Scarlett 2i2".into();
        cal
    }

    #[test]
    fn a_project_round_trips() {
        let root = temp_root("roundtrip");
        let store = ProjectStore::new(&root);

        let project = store.create("Hammersmith Apollo").unwrap();
        assert_eq!(project.dir, "Hammersmith Apollo");

        let back = store.open("Hammersmith Apollo").unwrap();
        assert_eq!(back.name, "Hammersmith Apollo");
        assert_eq!(back.id, project.id);

        let list = store.list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].show_count, 0);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_show_round_trips_with_its_whole_configuration() {
        let root = temp_root("show-roundtrip");
        let store = ProjectStore::new(&root);
        store.create("Tour").unwrap();

        let settings = Settings {
            device: Some("Scarlett 2i2".into()),
            sample_rate: Some(48_000),
            generator_channel: 3,
            layout: serde_json::json!({ "cols": 12, "tiles": [] }),
            ..Settings::default()
        };

        let show = Show::capture("FOH system", &settings, Some(a_calibration()));
        store.save_show("Tour", &show).unwrap();

        let back = store.load_show("Tour", &show.id).unwrap();
        assert_eq!(back.name, "FOH system");
        assert_eq!(back.device.as_deref(), Some("Scarlett 2i2"));
        assert_eq!(back.sample_rate, Some(48_000));
        assert_eq!(back.generator_channel, 3);
        assert_eq!(back.layout["cols"], 12);

        let shows = store.shows("Tour").unwrap();
        assert_eq!(shows.len(), 1);
        assert_eq!(shows[0].name, "FOH system");

        std::fs::remove_dir_all(&root).ok();
    }

    /// The invariant from `docs/tuning.md` §1.1. A show records the calibration it saw
    /// and there is no way to get it back out into the engine.
    #[test]
    fn a_show_records_a_calibration_but_restore_cannot_return_one() {
        let settings = Settings::default();
        let show = Show::capture("FOH", &settings, Some(a_calibration()));

        assert!(
            show.calibration_snapshot.is_some(),
            "the snapshot is what lets a stored trace say what it was measured against"
        );

        // `ShowRestore` has no calibration field at all — this test exists to fail at
        // compile time if one is ever added.
        let restored = show.restore();
        let ShowRestore {
            engine: _,
            transfer: _,
            generator: _,
            generator_channel: _,
            reference: _,
            host: _,
            device: _,
            sample_rate: _,
            layout: _,
        } = restored;
    }

    /// AGENTS.md §4.8, carried into show loading: the level comes back, the signal
    /// does not. Loading a show with a system live must not start pink noise.
    #[test]
    fn restoring_a_show_never_restores_a_running_generator() {
        let settings = Settings {
            generator: GeneratorConfig {
                signal: Signal::Pink,
                level_dbfs: -12.0,
                high_pass_hz: Some(40.0),
                low_pass_hz: None,
            },
            ..Settings::default()
        };

        let show = Show::capture("Pink noise show", &settings, None);
        assert!(matches!(show.generator.signal, Signal::Pink), "the show remembers it");

        let restored = show.restore();
        assert!(
            matches!(restored.generator.signal, Signal::Off),
            "a show must not put a signal into a PA when it loads"
        );
        assert_eq!(restored.generator.level_dbfs, -12.0, "the level is still restored");
        assert_eq!(restored.generator.high_pass_hz, Some(40.0));
    }

    #[test]
    fn renaming_a_project_moves_it_and_refuses_a_collision() {
        let root = temp_root("rename");
        let store = ProjectStore::new(&root);
        store.create("Old Name").unwrap();
        store.create("Taken").unwrap();

        let renamed = store.rename("Old Name", "New Name").unwrap();
        assert_eq!(renamed.name, "New Name");
        assert_eq!(renamed.dir, "New Name");
        assert!(root.join("New Name").exists());
        assert!(!root.join("Old Name").exists());

        assert!(store.rename("New Name", "Taken").is_err());
        assert!(root.join("New Name").exists(), "a refused rename changes nothing");

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn deleting_moves_rather_than_unlinks() {
        let root = temp_root("delete");
        let store = ProjectStore::new(&root);
        store.create("Doomed").unwrap();
        let show = Show::capture("A show", &Settings::default(), None);
        store.save_show("Doomed", &show).unwrap();

        let moved = store.delete_show("Doomed", &show.id).unwrap();
        assert!(moved.exists(), "the show file still exists after deletion");
        assert!(store.shows("Doomed").unwrap().is_empty());

        let moved = store.delete("Doomed").unwrap();
        assert!(moved.exists(), "the project directory still exists after deletion");
        assert!(store.list().is_empty(), "and it is not listed");

        std::fs::remove_dir_all(&root).ok();
    }

    /// A project name arrives from the frontend and is used to build a path. It must
    /// not be able to reach outside the root.
    #[test]
    fn a_project_name_cannot_escape_the_root() {
        assert_eq!(sanitise_name("../../etc/passwd"), "-..-etc-passwd");
        assert_eq!(sanitise_name(".."), "");
        assert_eq!(sanitise_name("."), "");
        assert_eq!(sanitise_name("/"), "-");
        assert!(!sanitise_name("C:\\Windows").contains(':'));
        assert!(!sanitise_name("a/b").contains('/'));
        assert!(!sanitise_name("a\\b").contains('\\'));

        let root = temp_root("escape");
        let store = ProjectStore::new(&root);
        // Whatever this resolves to, it stays under the root.
        let dir = store.project_dir("../../etc").unwrap();
        assert!(dir.starts_with(&root));

        std::fs::remove_dir_all(&root).ok();
    }

    /// Windows silently strips trailing dots and spaces and reserves device names.
    /// Both produce a project that saves here and cannot be opened there.
    #[test]
    fn names_are_safe_on_windows_too() {
        assert_eq!(sanitise_name("Show."), "Show");
        assert_eq!(sanitise_name("Show   "), "Show");
        assert_eq!(sanitise_name("CON"), "CON_");
        assert_eq!(sanitise_name("con"), "con_");
        assert_eq!(sanitise_name("PRN"), "PRN_");
        assert_eq!(sanitise_name("Control"), "Control", "only exact matches are reserved");
        assert_eq!(sanitise_name(""), "");
        assert_eq!(sanitise_name("   "), "");
    }

    #[test]
    fn a_show_id_is_readable_and_unique_per_name() {
        let id = make_id("FOH System — Main PA");
        assert!(id.starts_with("foh-system-main-pa-"), "got {id}");
        assert_eq!(sanitise_id(&id), id, "an id we generate is already filename-safe");

        // A name with nothing usable in it still produces a valid id.
        assert!(make_id("???").starts_with("show-"));
    }

    #[test]
    fn an_unreadable_show_is_skipped_and_left_on_disk() {
        let root = temp_root("broken-show");
        let store = ProjectStore::new(&root);
        store.create("Project").unwrap();
        let good = Show::capture("Good", &Settings::default(), None);
        store.save_show("Project", &good).unwrap();

        let broken = root.join("Project").join(SHOWS_DIR).join("broken.json");
        std::fs::write(&broken, b"{ not json").unwrap();

        let shows = store.shows("Project").unwrap();
        assert_eq!(shows.len(), 1, "the good one still lists");
        assert!(broken.exists(), "the broken one is not deleted — it is someone's work");

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn updating_a_show_keeps_its_identity() {
        let mut show = Show::capture("FOH", &Settings::default(), None);
        let id = show.id.clone();
        let created = show.created.clone();
        show.notes = "kept".into();

        let settings = Settings {
            device: Some("Different interface".into()),
            ..Settings::default()
        };
        show.update_from(&settings, None);

        assert_eq!(show.id, id);
        assert_eq!(show.created, created);
        assert_eq!(show.name, "FOH");
        assert_eq!(show.notes, "kept");
        assert_eq!(show.device.as_deref(), Some("Different interface"));
    }
}
