//! Microphone calibration against a hardware acoustic calibrator.
//!
//! A calibrator is a small cylinder that fits over the capsule and produces a
//! known sound pressure at a known frequency — 94 dB or 114 dB at 1 kHz, almost
//! always. Fit it, measure, and the difference between the level the converter
//! reports and the level the calibrator produces is a single offset that turns
//! dBFS into dB SPL for that microphone, on that input, at that gain.
//!
//! ```text
//! offset_db = reference_spl_db − measured_dbfs
//! spl_db    = dbfs + offset_db
//! ```
//!
//! That is the whole calculation. The rest of this module is about refusing to
//! accept a bad one, because a calibration is trusted silently for the rest of
//! the measurement and a mistake here is invisible afterwards: everything is
//! self-consistent and uniformly wrong. So a run has to satisfy four things
//! before it can be accepted — a stable level, the right frequency, no
//! clipping, and a tone well clear of the noise floor. See
//! [`CalibrationStatus`].
//!
//! ## What this cannot check
//!
//! That the calibrator is in calibration itself, that it is properly seated on
//! the capsule, or that the input gain is not changed afterwards. Changing the
//! preamp gain by one click invalidates the offset completely and nothing here
//! can detect it — which is why [`Calibration`] records the device and channel
//! it was taken on, and the app warns when either changes.

use serde::{Deserialize, Serialize};

/// A calibrator's published output.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CalibrationTarget {
    /// Sound pressure level the calibrator produces, dB SPL.
    pub level_db: f64,
    /// Frequency it produces it at, Hz.
    pub frequency_hz: f64,
}

impl Default for CalibrationTarget {
    fn default() -> Self {
        CalibrationTarget {
            level_db: 94.0,
            frequency_hz: 1000.0,
        }
    }
}

/// The outputs found on essentially every calibrator sold.
///
/// 94 dB is 1 pascal, which is why it is the common one. 114 dB is the same
/// instrument's high setting, used when 94 dB would sit too close to the noise
/// floor of a quiet measurement chain. 250 Hz appears on some class 1 units for
/// checking C-weighting.
pub const STANDARD_TARGETS: &[CalibrationTarget] = &[
    CalibrationTarget {
        level_db: 94.0,
        frequency_hz: 1000.0,
    },
    CalibrationTarget {
        level_db: 114.0,
        frequency_hz: 1000.0,
    },
    CalibrationTarget {
        level_db: 94.0,
        frequency_hz: 250.0,
    },
];

/// An accepted calibration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Calibration {
    /// Add this to a dBFS level to get dB SPL.
    pub offset_db: f64,
    /// What the calibrator was set to.
    pub target: CalibrationTarget,
    /// What the input actually read during the run.
    pub measured_dbfs: f64,
    /// Which input this was taken on. A calibration is only valid for the
    /// device, channel and gain it was measured on.
    #[serde(default)]
    pub device: String,
    #[serde(default)]
    pub channel: usize,
    /// RFC 3339 timestamp, filled in by the app layer.
    #[serde(default)]
    pub taken_at: String,
}

impl Calibration {
    pub fn new(target: CalibrationTarget, measured_dbfs: f64) -> Self {
        Calibration {
            offset_db: target.level_db - measured_dbfs,
            target,
            measured_dbfs,
            device: String::new(),
            channel: 0,
            taken_at: String::new(),
        }
    }

    /// dBFS → dB SPL.
    pub fn spl_from_dbfs(&self, dbfs: f64) -> f64 {
        dbfs + self.offset_db
    }

    /// The SPL a full-scale signal would correspond to.
    ///
    /// This is the number worth showing next to a calibration: it says where
    /// the measurement runs out of headroom. A chain calibrated so that full
    /// scale is 120 dB SPL cannot measure a snare at the drummer's ear.
    pub fn full_scale_spl_db(&self) -> f64 {
        self.offset_db
    }
}

/// Why a calibration run is not yet acceptable — or that it is.
#[derive(Debug, Clone, PartialEq, Serialize)]
// `rename_all` renames the *variants*; the fields inside a struct variant need
// `rename_all_fields` as well. Without it the UI reads `levelDbfs` off a
// payload carrying `level_dbfs`, gets `undefined`, and the dialog throws.
#[serde(
    tag = "state",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum CalibrationStatus {
    /// Not enough signal observed yet.
    Settling { progress: f64 },
    /// The level is still moving. Usually the calibrator is not seated, or it
    /// has only just been switched on.
    Unstable { spread_db: f64 },
    /// The dominant tone is not where the calibrator says it should be. Almost
    /// always the wrong target selected, or the calibrator on its other setting.
    WrongFrequency { measured_hz: f64, expected_hz: f64 },
    /// The input is clipping, so the measured level is a lower bound and the
    /// resulting offset would be wrong in the dangerous direction.
    Clipping,
    /// The tone is barely above the input's own noise. Either the calibrator is
    /// not running or the input gain is far too low.
    TooQuiet { level_dbfs: f64 },
    /// Good to accept.
    Ready {
        measured_dbfs: f64,
        spread_db: f64,
        offset_db: f64,
    },
}

impl CalibrationStatus {
    pub fn is_ready(&self) -> bool {
        matches!(self, CalibrationStatus::Ready { .. })
    }
}

/// One observation, produced once per processing block by the engine.
#[derive(Debug, Clone, Copy)]
pub struct CalibrationSample {
    /// Unweighted block level, dBFS.
    pub level_dbfs: f64,
    /// Frequency of the strongest component, Hz. `None` if not yet known.
    pub dominant_hz: Option<f64>,
    /// Block sample peak, linear.
    pub peak: f64,
    pub seconds: f64,
}

/// How long a run must observe a steady tone before it will report `Ready`.
pub const SETTLE_SECONDS: f64 = 3.0;
/// Largest spread between the highest and lowest block level, in dB, that still
/// counts as steady. A properly seated calibrator sits far inside this.
pub const MAX_SPREAD_DB: f64 = 0.5;
/// How far the measured tone may sit from the target, as a fraction. ±5% is
/// wide enough for a cheap calibrator and a converter running slightly off
/// nominal, and far narrower than the gap between 250 Hz and 1 kHz.
pub const MAX_FREQUENCY_ERROR: f64 = 0.05;
/// Below this, a calibrator is not what is being measured.
pub const MIN_LEVEL_DBFS: f64 = -60.0;

/// A calibration in progress.
pub struct CalibrationRun {
    target: CalibrationTarget,
    /// Block levels inside the settle window, oldest first.
    window: Vec<(f64, f64)>,
    observed: f64,
    energy: f64,
    seconds: f64,
    dominant_hz: Option<f64>,
    clipped: bool,
}

impl CalibrationRun {
    pub fn new(target: CalibrationTarget) -> Self {
        CalibrationRun {
            target,
            window: Vec::new(),
            observed: 0.0,
            energy: 0.0,
            seconds: 0.0,
            dominant_hz: None,
            clipped: false,
        }
    }

    pub fn target(&self) -> CalibrationTarget {
        self.target
    }

    pub fn push(&mut self, s: CalibrationSample) {
        if s.seconds <= 0.0 || !s.level_dbfs.is_finite() {
            return;
        }
        self.observed += s.seconds;
        if s.peak >= 1.0 {
            self.clipped = true;
        }
        if let Some(hz) = s.dominant_hz {
            self.dominant_hz = Some(hz);
        }

        // Keep the trailing SETTLE_SECONDS of block levels, and integrate the
        // energy over the same span. Energy rather than a mean of decibels,
        // for the reason set out in `leq`.
        let ms = 10f64.powf(s.level_dbfs / 10.0);
        self.window.push((s.level_dbfs, s.seconds));
        self.energy += ms * s.seconds;
        self.seconds += s.seconds;

        let mut i = 0;
        while self.seconds > SETTLE_SECONDS && i < self.window.len() {
            let (lvl, dur) = self.window[i];
            if self.seconds - dur < SETTLE_SECONDS {
                break;
            }
            self.energy -= 10f64.powf(lvl / 10.0) * dur;
            self.seconds -= dur;
            i += 1;
        }
        if i > 0 {
            self.window.drain(..i);
        }
    }

    /// Energy-mean level over the settle window, dBFS.
    pub fn measured_dbfs(&self) -> f64 {
        if self.seconds <= 0.0 {
            return f64::NEG_INFINITY;
        }
        10.0 * (self.energy / self.seconds).log10()
    }

    fn spread_db(&self) -> f64 {
        if self.window.is_empty() {
            return f64::INFINITY;
        }
        let mut lo = f64::INFINITY;
        let mut hi = f64::NEG_INFINITY;
        for &(lvl, _) in &self.window {
            lo = lo.min(lvl);
            hi = hi.max(lvl);
        }
        hi - lo
    }

    pub fn status(&self) -> CalibrationStatus {
        if self.clipped {
            return CalibrationStatus::Clipping;
        }
        if self.observed < SETTLE_SECONDS {
            return CalibrationStatus::Settling {
                progress: (self.observed / SETTLE_SECONDS).clamp(0.0, 1.0),
            };
        }

        let measured = self.measured_dbfs();
        if measured < MIN_LEVEL_DBFS {
            return CalibrationStatus::TooQuiet {
                level_dbfs: measured,
            };
        }
        if let Some(hz) = self.dominant_hz {
            let err = (hz - self.target.frequency_hz).abs() / self.target.frequency_hz;
            if err > MAX_FREQUENCY_ERROR {
                return CalibrationStatus::WrongFrequency {
                    measured_hz: hz,
                    expected_hz: self.target.frequency_hz,
                };
            }
        }
        let spread = self.spread_db();
        if spread > MAX_SPREAD_DB {
            return CalibrationStatus::Unstable { spread_db: spread };
        }

        CalibrationStatus::Ready {
            measured_dbfs: measured,
            spread_db: spread,
            offset_db: self.target.level_db - measured,
        }
    }

    /// Take the calibration, if the run is good enough to give one.
    pub fn accept(&self) -> Option<Calibration> {
        match self.status() {
            CalibrationStatus::Ready { measured_dbfs, .. } => {
                Some(Calibration::new(self.target, measured_dbfs))
            }
            _ => None,
        }
    }

    pub fn reset(&mut self) {
        self.window.clear();
        self.observed = 0.0;
        self.energy = 0.0;
        self.seconds = 0.0;
        self.dominant_hz = None;
        self.clipped = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every field the dialog reads off a status, in the casing it reads them.
    ///
    /// `CalibrationDialog.tsx` calls `.toFixed()` on these directly, so a field
    /// arriving under its Rust name is not a cosmetic mismatch — it is
    /// `undefined.toFixed()`, which throws during render and, with no error
    /// boundary above it, unmounts the whole app. The `ready` variant matters
    /// most: that is the *successful* path, so getting it wrong means a real
    /// calibrator takes the app down at the moment it is about to work.
    #[test]
    fn every_status_variant_crosses_to_the_ui_in_camel_case() {
        let cases = [
            (
                CalibrationStatus::Unstable { spread_db: 0.4 },
                vec!["spreadDb"],
            ),
            (
                CalibrationStatus::WrongFrequency {
                    measured_hz: 250.0,
                    expected_hz: 1000.0,
                },
                vec!["measuredHz", "expectedHz"],
            ),
            (
                CalibrationStatus::TooQuiet { level_dbfs: -200.0 },
                vec!["levelDbfs"],
            ),
            (
                CalibrationStatus::Ready {
                    measured_dbfs: -26.0,
                    spread_db: 0.05,
                    offset_db: 120.0,
                },
                vec!["measuredDbfs", "spreadDb", "offsetDb"],
            ),
        ];

        for (status, keys) in cases {
            let json = serde_json::to_value(&status).expect("serialises");
            for key in keys {
                assert!(json.get(key).is_some(), "missing {key} in {json}");
            }
            let snake: Vec<&str> = json
                .as_object()
                .expect("an object")
                .keys()
                .filter(|k| k.contains('_'))
                .map(|k| k.as_str())
                .collect();
            assert!(snake.is_empty(), "snake_case survived: {snake:?} in {json}");
        }
    }

    fn steady(run: &mut CalibrationRun, level_dbfs: f64, seconds: f64) {
        let block = 0.02;
        let n = (seconds / block).round() as usize;
        for _ in 0..n {
            run.push(CalibrationSample {
                level_dbfs,
                dominant_hz: Some(run.target.frequency_hz),
                peak: 10f64.powf(level_dbfs / 20.0) * 1.414,
                seconds: block,
            });
        }
    }

    #[test]
    fn offset_is_the_difference_between_reference_and_measurement() {
        let c = Calibration::new(CalibrationTarget::default(), -26.0);
        assert!((c.offset_db - 120.0).abs() < 1e-12);
        assert!((c.spl_from_dbfs(-26.0) - 94.0).abs() < 1e-12);
        assert!((c.spl_from_dbfs(0.0) - 120.0).abs() < 1e-12);
        assert!((c.full_scale_spl_db() - 120.0).abs() < 1e-12);
    }

    #[test]
    fn a_calibrated_chain_reads_the_calibrator_back() {
        // The round trip that matters: calibrate at 94 dB, then a signal at the
        // same dBFS must read 94 dB SPL.
        let mut run = CalibrationRun::new(CalibrationTarget::default());
        steady(&mut run, -26.0, 4.0);
        let cal = run.accept().expect("run should be acceptable");
        assert!((cal.spl_from_dbfs(-26.0) - 94.0).abs() < 1e-9);
        // And 20 dB more signal must read 20 dB more SPL.
        assert!((cal.spl_from_dbfs(-6.0) - 114.0).abs() < 1e-9);
    }

    #[test]
    fn a_run_settles_before_it_is_ready() {
        let mut run = CalibrationRun::new(CalibrationTarget::default());
        steady(&mut run, -26.0, 1.0);
        assert!(matches!(
            run.status(),
            CalibrationStatus::Settling { .. }
        ));
        assert!(run.accept().is_none());
        steady(&mut run, -26.0, 3.0);
        assert!(run.status().is_ready());
    }

    #[test]
    fn a_wandering_level_is_rejected() {
        let mut run = CalibrationRun::new(CalibrationTarget::default());
        let block = 0.02;
        for i in 0..300 {
            // ±1.5 dB wander: a calibrator that is not seated.
            let level = -26.0 + 1.5 * ((i as f64) / 10.0).sin();
            run.push(CalibrationSample {
                level_dbfs: level,
                dominant_hz: Some(1000.0),
                peak: 0.05,
                seconds: block,
            });
        }
        match run.status() {
            CalibrationStatus::Unstable { spread_db } => assert!(spread_db > MAX_SPREAD_DB),
            other => panic!("expected Unstable, got {other:?}"),
        }
        assert!(run.accept().is_none());
    }

    #[test]
    fn the_wrong_calibrator_setting_is_caught() {
        // Target says 1 kHz, calibrator is actually on 250 Hz.
        let mut run = CalibrationRun::new(CalibrationTarget::default());
        for _ in 0..300 {
            run.push(CalibrationSample {
                level_dbfs: -26.0,
                dominant_hz: Some(250.0),
                peak: 0.05,
                seconds: 0.02,
            });
        }
        match run.status() {
            CalibrationStatus::WrongFrequency {
                measured_hz,
                expected_hz,
            } => {
                assert_eq!(measured_hz, 250.0);
                assert_eq!(expected_hz, 1000.0);
            }
            other => panic!("expected WrongFrequency, got {other:?}"),
        }
    }

    #[test]
    fn a_small_frequency_error_is_tolerated() {
        let mut run = CalibrationRun::new(CalibrationTarget::default());
        for _ in 0..300 {
            run.push(CalibrationSample {
                level_dbfs: -26.0,
                dominant_hz: Some(1012.0),
                peak: 0.05,
                seconds: 0.02,
            });
        }
        assert!(run.status().is_ready(), "1.2% off should be acceptable");
    }

    #[test]
    fn clipping_blocks_a_calibration() {
        let mut run = CalibrationRun::new(CalibrationTarget::default());
        steady(&mut run, -26.0, 4.0);
        assert!(run.status().is_ready());
        run.push(CalibrationSample {
            level_dbfs: -1.0,
            dominant_hz: Some(1000.0),
            peak: 1.0,
            seconds: 0.02,
        });
        assert_eq!(run.status(), CalibrationStatus::Clipping);
        assert!(run.accept().is_none());
    }

    #[test]
    fn a_silent_input_is_reported_as_too_quiet() {
        let mut run = CalibrationRun::new(CalibrationTarget::default());
        steady(&mut run, -90.0, 4.0);
        match run.status() {
            CalibrationStatus::TooQuiet { level_dbfs } => assert!(level_dbfs < MIN_LEVEL_DBFS),
            other => panic!("expected TooQuiet, got {other:?}"),
        }
    }

    #[test]
    fn the_high_setting_gives_an_offset_twenty_decibels_lower() {
        // Same microphone and gain, calibrator switched from 94 to 114 dB: the
        // input reads 20 dB hotter and the offset must drop by exactly 20.
        let mut low = CalibrationRun::new(STANDARD_TARGETS[0]);
        steady(&mut low, -26.0, 4.0);
        let mut high = CalibrationRun::new(STANDARD_TARGETS[1]);
        steady(&mut high, -6.0, 4.0);
        let a = low.accept().unwrap();
        let b = high.accept().unwrap();
        assert!(
            (a.offset_db - b.offset_db).abs() < 1e-9,
            "the same chain gave {} and {}",
            a.offset_db,
            b.offset_db
        );
    }

    #[test]
    fn resetting_starts_the_run_over() {
        let mut run = CalibrationRun::new(CalibrationTarget::default());
        steady(&mut run, -26.0, 4.0);
        assert!(run.status().is_ready());
        run.reset();
        assert!(matches!(run.status(), CalibrationStatus::Settling { .. }));
    }
}
