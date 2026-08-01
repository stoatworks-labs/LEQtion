//! Persisted settings: what to measure, what to show, and the calibration.
//!
//! One JSON file in the platform config directory. It is written whole on every
//! change and read once at startup; there is no migration machinery because
//! there is nothing here worth migrating — every field has a default, and a file
//! that fails to parse is replaced rather than mourned. Losing a tile layout is
//! an annoyance. Refusing to start because a settings file is from a newer build
//! is worse.
//!
//! The calibration is the exception, and it is the reason this file is written
//! atomically: it takes a calibrator, a quiet room and a couple of minutes to
//! reproduce, and a half-written file that loses it silently would be found the
//! next time someone needed a number they could defend.

use std::path::PathBuf;

use leqtion_dsp::calibration::Calibration;
use leqtion_dsp::engine::EngineConfig;
use leqtion_dsp::generator::GeneratorConfig;
use leqtion_dsp::transfer::TransferConfig;

use crate::session::ReferenceSource;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    #[serde(default)]
    pub engine: EngineConfig,
    #[serde(default)]
    pub transfer: TransferConfig,
    /// The generator is deliberately **not** restored to its last signal on
    /// launch — see `Default`. Only its level and shaping are remembered.
    #[serde(default)]
    pub generator: GeneratorConfig,
    /// Output channel the generator drives.
    #[serde(default)]
    pub generator_channel: usize,
    #[serde(default)]
    pub reference: ReferenceSource,
    /// Last input used, so the app comes back where it was left.
    #[serde(default)]
    pub host: Option<String>,
    #[serde(default)]
    pub device: Option<String>,
    #[serde(default)]
    pub sample_rate: Option<u32>,
    /// Calibration per device name. Keyed by device because a calibration
    /// belongs to a microphone and preamp, not to the app — plugging in a
    /// different interface must not inherit the last one's offset.
    #[serde(default)]
    pub calibrations: Vec<Calibration>,
    /// Tile layout, owned entirely by the frontend. The backend stores it and
    /// never looks inside; that keeps the layout format a UI concern and stops
    /// this file becoming a second place the UI is defined.
    #[serde(default)]
    pub layout: serde_json::Value,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            engine: EngineConfig::default(),
            transfer: TransferConfig::default(),
            // Signal::Off by default, and it stays off across a restart even if
            // the last session was generating. Opening a measurement app should
            // never put pink noise into a PA before anyone has touched anything.
            generator: GeneratorConfig::default(),
            generator_channel: 0,
            reference: ReferenceSource::default(),
            host: None,
            device: None,
            sample_rate: None,
            calibrations: Vec::new(),
            layout: serde_json::Value::Null,
        }
    }
}

impl Settings {
    pub fn calibration_for(&self, device: &str) -> Option<&Calibration> {
        self.calibrations.iter().find(|c| c.device == device)
    }

    /// Store a calibration, replacing any previous one for the same device.
    pub fn set_calibration(&mut self, cal: Calibration) {
        self.calibrations.retain(|c| c.device != cal.device);
        self.calibrations.push(cal);
    }

    pub fn load(path: &PathBuf) -> Self {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Settings::default();
        };
        match serde_json::from_str(&text) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("settings at {} are unreadable ({e}); starting fresh", path.display());
                Settings::default()
            }
        }
    }

    /// Write via a temporary file and rename.
    ///
    /// A rename is atomic on every platform this ships to, so an interrupted
    /// write leaves the previous settings intact rather than a truncated file
    /// that parses as nothing and quietly discards a calibration.
    pub fn save(&self, path: &PathBuf) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&tmp, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use leqtion_dsp::calibration::CalibrationTarget;

    #[test]
    fn defaults_round_trip() {
        let s = Settings::default();
        let text = serde_json::to_string(&s).unwrap();
        let back: Settings = serde_json::from_str(&text).unwrap();
        assert_eq!(back.calibrations.len(), 0);
        assert!(back.device.is_none());
    }

    #[test]
    fn an_unreadable_file_gives_defaults_rather_than_an_error() {
        let dir = std::env::temp_dir().join("leqtion-test-settings");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("broken.json");
        std::fs::write(&path, b"{ this is not json").unwrap();
        let s = Settings::load(&path);
        assert!(s.calibrations.is_empty());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_missing_file_gives_defaults() {
        let path = std::env::temp_dir().join("leqtion-does-not-exist-12345.json");
        let s = Settings::load(&path);
        assert!(s.device.is_none());
    }

    #[test]
    fn calibrations_are_keyed_by_device() {
        let mut s = Settings::default();
        let mut a = Calibration::new(CalibrationTarget::default(), -26.0);
        a.device = "Scarlett 2i2".into();
        let mut b = Calibration::new(CalibrationTarget::default(), -30.0);
        b.device = "Built-in Microphone".into();
        s.set_calibration(a);
        s.set_calibration(b);
        assert_eq!(s.calibrations.len(), 2);

        // Recalibrating the same device replaces rather than accumulates.
        let mut again = Calibration::new(CalibrationTarget::default(), -24.0);
        again.device = "Scarlett 2i2".into();
        s.set_calibration(again);
        assert_eq!(s.calibrations.len(), 2);
        assert!(
            (s.calibration_for("Scarlett 2i2").unwrap().measured_dbfs - -24.0).abs() < 1e-12
        );
        assert!(s.calibration_for("nothing plugged in").is_none());
    }

    #[test]
    fn saving_and_loading_preserves_a_calibration() {
        let dir = std::env::temp_dir().join("leqtion-test-settings");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("roundtrip.json");

        let mut s = Settings::default();
        let mut cal = Calibration::new(CalibrationTarget::default(), -26.0);
        cal.device = "Scarlett 2i2".into();
        s.set_calibration(cal);
        s.save(&path).unwrap();

        let back = Settings::load(&path);
        let got = back.calibration_for("Scarlett 2i2").expect("calibration lost");
        assert!((got.offset_db - 120.0).abs() < 1e-9);
        std::fs::remove_file(&path).ok();
    }
}
