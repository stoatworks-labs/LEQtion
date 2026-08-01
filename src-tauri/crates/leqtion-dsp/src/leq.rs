//! Equivalent continuous sound level (LEQ).
//!
//! LEQ is the level of the steady sound carrying the same energy as the actual
//! sound over some period:
//!
//! ```text
//! Leq,T = 10 · log10( (1/T) ∫ p²(t) dt / p₀² )
//! ```
//!
//! So an accumulator is a sum of energy and a sum of time, and nothing else.
//! There is no time weighting in an LEQ — Fast and Slow belong to the SPL
//! readout, not here. A common mistake is to average a Fast-weighted level over
//! a window and call it an LEQ; that answer is close but not the same, and it
//! is wrong in a way that grows with how peaky the signal is.
//!
//! Two window shapes, because both are asked for:
//!
//! - [`LeqWindow::Sliding`] — the last N seconds, continuously. This is what a
//!   "short-term LEQ" or a 5-minute rolling limit means, and it is what a
//!   venue's noise limiter is usually watching.
//! - [`LeqWindow::Elapsed`] — everything since the last reset. The show LEQ.
//!
//! The weighting is a property of the *signal fed in*, not of this module: the
//! engine runs the samples through an A, C or Z filter first and hands the
//! resulting mean square over. That keeps the integral honest — see
//! [`crate::weighting`] for why the filtering happens in the time domain.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

use crate::spl::{mean_square_to_dbfs, SILENCE_DBFS};
use crate::weighting::Weighting;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum LeqWindow {
    /// A rolling window of the last `seconds`.
    Sliding { seconds: f64 },
    /// Everything since the last reset.
    Elapsed,
}

impl LeqWindow {
    pub fn label(self) -> String {
        match self {
            LeqWindow::Elapsed => "elapsed".to_string(),
            LeqWindow::Sliding { seconds } => format_duration(seconds),
        }
    }
}

/// Render a window length the way a sound engineer would say it.
pub fn format_duration(seconds: f64) -> String {
    if seconds < 1.0 {
        format!("{:.0} ms", seconds * 1000.0)
    } else if seconds < 60.0 {
        let s = if (seconds.fract()).abs() < 1e-9 {
            format!("{:.0}", seconds)
        } else {
            format!("{:.1}", seconds)
        };
        format!("{s} s")
    } else if seconds < 3600.0 {
        let m = seconds / 60.0;
        if (m.fract()).abs() < 1e-9 {
            format!("{m:.0} min")
        } else {
            format!("{m:.1} min")
        }
    } else {
        let h = seconds / 3600.0;
        if (h.fract()).abs() < 1e-9 {
            format!("{h:.0} h")
        } else {
            format!("{h:.2} h")
        }
    }
}

/// How a single user-defined LEQ readout is configured.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LeqSpec {
    /// Stable identifier, so a tile can keep pointing at the same LEQ across
    /// reconfiguration.
    pub id: String,
    /// What the user called it. Empty means "derive one from the settings".
    #[serde(default)]
    pub label: String,
    pub weighting: Weighting,
    pub window: LeqWindow,
}

impl LeqSpec {
    /// The conventional notation: `LAeq,5min`, `LCeq`, `LZeq,125ms`.
    pub fn derived_label(&self) -> String {
        match self.window {
            LeqWindow::Elapsed => format!("L{}eq", self.weighting.label()),
            LeqWindow::Sliding { seconds } => {
                format!("L{}eq,{}", self.weighting.label(), compact_duration(seconds))
            }
        }
    }

    pub fn display_label(&self) -> String {
        if self.label.trim().is_empty() {
            self.derived_label()
        } else {
            self.label.clone()
        }
    }
}

/// The tight form used inside a level name: `5min`, `125ms`, `1h`.
fn compact_duration(seconds: f64) -> String {
    if seconds < 1.0 {
        format!("{:.0}ms", seconds * 1000.0)
    } else if seconds < 60.0 {
        format!("{}s", trim_number(seconds))
    } else if seconds < 3600.0 {
        format!("{}min", trim_number(seconds / 60.0))
    } else {
        format!("{}h", trim_number(seconds / 3600.0))
    }
}

fn trim_number(v: f64) -> String {
    if (v.fract()).abs() < 1e-9 {
        format!("{v:.0}")
    } else {
        format!("{v:.2}")
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_string()
    }
}

/// How often to rebuild the running sums from the ring exactly.
///
/// Adding and subtracting a running total over a long measurement accumulates
/// rounding error. It is small — around 1e-13 relative after a full day — but
/// rebuilding costs nothing next to being unable to say the total is exact.
const RESYNC_EVERY: u64 = 65_536;

#[derive(Debug, Clone, Copy)]
struct Slice {
    /// Mean square × duration, i.e. energy.
    energy: f64,
    seconds: f64,
}

/// One running LEQ.
#[derive(Debug, Clone)]
pub struct LeqAccumulator {
    spec: LeqSpec,
    /// Only populated for a sliding window.
    ring: VecDeque<Slice>,
    energy: f64,
    seconds: f64,
    /// Total time observed since reset, which for a sliding window keeps
    /// running after the window is full.
    elapsed: f64,
    pops: u64,
}

impl LeqAccumulator {
    pub fn new(spec: LeqSpec) -> Self {
        LeqAccumulator {
            spec,
            ring: VecDeque::new(),
            energy: 0.0,
            seconds: 0.0,
            elapsed: 0.0,
            pops: 0,
        }
    }

    pub fn spec(&self) -> &LeqSpec {
        &self.spec
    }

    /// Change the window or weighting in place.
    ///
    /// A change of weighting cannot be applied retroactively — the energy
    /// already accumulated was filtered differently — so it clears the history.
    /// Shortening a sliding window keeps the part of the history that still
    /// falls inside it, which is what a user dragging a slider expects.
    pub fn reconfigure(&mut self, spec: LeqSpec) {
        let weighting_changed = spec.weighting != self.spec.weighting;
        let was_elapsed = matches!(self.spec.window, LeqWindow::Elapsed);
        let now_elapsed = matches!(spec.window, LeqWindow::Elapsed);
        self.spec = spec;

        if weighting_changed || was_elapsed != now_elapsed {
            self.reset();
            return;
        }
        self.trim();
    }

    /// Feed one block: its mean square and how long it lasted.
    pub fn push(&mut self, mean_square: f64, seconds: f64) {
        if seconds <= 0.0 || !mean_square.is_finite() || mean_square < 0.0 {
            return;
        }
        self.elapsed += seconds;
        let energy = mean_square * seconds;

        match self.spec.window {
            LeqWindow::Elapsed => {
                self.energy += energy;
                self.seconds += seconds;
            }
            LeqWindow::Sliding { .. } => {
                self.ring.push_back(Slice { energy, seconds });
                self.energy += energy;
                self.seconds += seconds;
                self.trim();
            }
        }
    }

    fn trim(&mut self) {
        let LeqWindow::Sliding { seconds: window } = self.spec.window else {
            return;
        };
        let window = window.max(0.001);
        while self.seconds > window {
            let Some(front) = self.ring.front().copied() else {
                break;
            };
            // Stop before the window would be left short: it is better for the
            // window to be a fraction of a block long than a block short.
            if self.seconds - front.seconds < window {
                break;
            }
            self.ring.pop_front();
            self.energy -= front.energy;
            self.seconds -= front.seconds;
            self.pops += 1;
            if self.pops.is_multiple_of(RESYNC_EVERY) {
                self.resync();
            }
        }
    }

    fn resync(&mut self) {
        self.energy = self.ring.iter().map(|s| s.energy).sum();
        self.seconds = self.ring.iter().map(|s| s.seconds).sum();
    }

    /// The LEQ, in dBFS. Add the calibration offset to reach dB SPL.
    pub fn leq_dbfs(&self) -> f64 {
        if self.seconds <= 0.0 {
            return SILENCE_DBFS;
        }
        mean_square_to_dbfs(self.energy / self.seconds)
    }

    /// Seconds of signal currently inside the window.
    pub fn integrated_seconds(&self) -> f64 {
        self.seconds
    }

    /// Seconds since the last reset, whether or not they are still in the window.
    pub fn elapsed_seconds(&self) -> f64 {
        self.elapsed
    }

    /// 0.0 to 1.0 — how full a sliding window is. Always 1.0 for an elapsed LEQ
    /// once anything has been measured, because there is nothing left to fill.
    pub fn fill(&self) -> f64 {
        match self.spec.window {
            LeqWindow::Elapsed => {
                if self.seconds > 0.0 {
                    1.0
                } else {
                    0.0
                }
            }
            LeqWindow::Sliding { seconds } if seconds > 0.0 => (self.seconds / seconds).min(1.0),
            LeqWindow::Sliding { .. } => 0.0,
        }
    }

    pub fn reset(&mut self) {
        self.ring.clear();
        self.energy = 0.0;
        self.seconds = 0.0;
        self.elapsed = 0.0;
        self.pops = 0;
    }
}

/// A snapshot of one LEQ, as sent to the UI.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LeqReading {
    pub id: String,
    pub label: String,
    pub weighting: Weighting,
    /// LEQ in dB SPL once calibrated, dBFS if not.
    pub value: f64,
    /// True when `value` is a real SPL rather than a full-scale level.
    pub calibrated: bool,
    pub elapsed_seconds: f64,
    pub integrated_seconds: f64,
    pub fill: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sliding(seconds: f64) -> LeqSpec {
        LeqSpec {
            id: "t".into(),
            label: String::new(),
            weighting: Weighting::A,
            window: LeqWindow::Sliding { seconds },
        }
    }

    fn elapsed() -> LeqSpec {
        LeqSpec {
            id: "t".into(),
            label: String::new(),
            weighting: Weighting::Z,
            window: LeqWindow::Elapsed,
        }
    }

    /// A steady signal has an LEQ equal to its own level, whatever the window.
    #[test]
    fn steady_signal_leq_equals_its_level() {
        let mut a = LeqAccumulator::new(elapsed());
        for _ in 0..1000 {
            a.push(0.5, 0.02);
        }
        assert!((a.leq_dbfs() - mean_square_to_dbfs(0.5)).abs() < 1e-9);
    }

    /// The property that makes LEQ worth computing: half the time at full
    /// energy is 3 dB below full, not 6, and not the average of the decibels.
    #[test]
    fn fifty_percent_duty_cycle_is_three_decibels_down() {
        let mut a = LeqAccumulator::new(elapsed());
        for i in 0..2000 {
            a.push(if i % 2 == 0 { 1.0 } else { 0.0 }, 0.01);
        }
        let expected = mean_square_to_dbfs(0.5);
        assert!(
            (a.leq_dbfs() - expected).abs() < 1e-9,
            "got {:.4}, expected {expected:.4}",
            a.leq_dbfs()
        );
    }

    #[test]
    fn averaging_decibels_would_give_a_different_answer() {
        // Guards against someone "simplifying" the accumulator into a mean of
        // levels. 90 dB for half the time and 60 dB for half is 87 dB, not 75.
        let mut a = LeqAccumulator::new(elapsed());
        let loud = 10f64.powf(9.0);
        let quiet = 10f64.powf(6.0);
        for i in 0..1000 {
            a.push(if i % 2 == 0 { loud } else { quiet }, 0.1);
        }
        let energy_mean = mean_square_to_dbfs((loud + quiet) / 2.0);
        let db_mean = (mean_square_to_dbfs(loud) + mean_square_to_dbfs(quiet)) / 2.0;
        assert!((a.leq_dbfs() - energy_mean).abs() < 1e-9);
        assert!(
            (a.leq_dbfs() - db_mean).abs() > 10.0,
            "the two means are supposed to differ substantially here"
        );
    }

    #[test]
    fn sliding_window_forgets_the_past() {
        let mut a = LeqAccumulator::new(sliding(1.0));
        // One second of loud, then two seconds of quiet.
        for _ in 0..100 {
            a.push(1.0, 0.01);
        }
        let loud = a.leq_dbfs();
        for _ in 0..200 {
            a.push(1e-6, 0.01);
        }
        let quiet = a.leq_dbfs();
        assert!((loud - mean_square_to_dbfs(1.0)).abs() < 0.2);
        assert!(
            (quiet - mean_square_to_dbfs(1e-6)).abs() < 0.2,
            "window still remembers the loud part: {quiet:.2} dB"
        );
    }

    #[test]
    fn sliding_window_holds_about_its_length() {
        let mut a = LeqAccumulator::new(sliding(5.0));
        for _ in 0..2000 {
            a.push(0.25, 0.01);
        }
        assert!(
            (a.integrated_seconds() - 5.0).abs() < 0.02,
            "window holds {:.3} s",
            a.integrated_seconds()
        );
        assert!((a.elapsed_seconds() - 20.0).abs() < 1e-6);
        assert!((a.fill() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn fill_reports_partial_windows() {
        let mut a = LeqAccumulator::new(sliding(10.0));
        for _ in 0..250 {
            a.push(0.25, 0.01);
        }
        assert!((a.fill() - 0.25).abs() < 0.01, "fill was {}", a.fill());
    }

    #[test]
    fn shortening_a_window_keeps_what_still_fits() {
        let mut a = LeqAccumulator::new(sliding(10.0));
        for _ in 0..1000 {
            a.push(0.25, 0.01);
        }
        a.reconfigure(sliding(2.0));
        assert!((a.integrated_seconds() - 2.0).abs() < 0.02);
        assert!((a.leq_dbfs() - mean_square_to_dbfs(0.25)).abs() < 1e-9);
    }

    #[test]
    fn changing_weighting_clears_history() {
        let mut a = LeqAccumulator::new(sliding(10.0));
        for _ in 0..500 {
            a.push(0.25, 0.01);
        }
        let mut spec = sliding(10.0);
        spec.weighting = Weighting::C;
        a.reconfigure(spec);
        assert_eq!(a.integrated_seconds(), 0.0);
        assert_eq!(a.leq_dbfs(), SILENCE_DBFS);
    }

    #[test]
    fn empty_accumulator_is_silent_not_nan() {
        let a = LeqAccumulator::new(elapsed());
        assert_eq!(a.leq_dbfs(), SILENCE_DBFS);
        assert_eq!(a.fill(), 0.0);
    }

    #[test]
    fn rubbish_input_is_ignored_rather_than_poisoning_the_total() {
        let mut a = LeqAccumulator::new(elapsed());
        a.push(0.5, 0.1);
        a.push(f64::NAN, 0.1);
        a.push(-1.0, 0.1);
        a.push(0.5, 0.0);
        assert!(a.leq_dbfs().is_finite());
        assert!((a.leq_dbfs() - mean_square_to_dbfs(0.5)).abs() < 1e-12);
        assert!((a.elapsed_seconds() - 0.1).abs() < 1e-12);
    }

    #[test]
    fn long_run_stays_accurate() {
        // Eight hours at 50 blocks a second, which is well past the resync
        // interval, so this exercises the rebuild path too.
        let mut a = LeqAccumulator::new(sliding(60.0));
        for _ in 0..(50 * 3600 * 8) {
            a.push(0.25, 0.02);
        }
        assert!(
            (a.leq_dbfs() - mean_square_to_dbfs(0.25)).abs() < 1e-9,
            "drifted to {:.9}",
            a.leq_dbfs()
        );
        assert!((a.integrated_seconds() - 60.0).abs() < 0.05);
    }

    #[test]
    fn derived_labels_read_like_level_names() {
        let cases = [
            (LeqWindow::Elapsed, Weighting::A, "LAeq"),
            (
                LeqWindow::Sliding { seconds: 300.0 },
                Weighting::A,
                "LAeq,5min",
            ),
            (
                LeqWindow::Sliding { seconds: 0.125 },
                Weighting::Z,
                "LZeq,125ms",
            ),
            (
                LeqWindow::Sliding { seconds: 3600.0 },
                Weighting::C,
                "LCeq,1h",
            ),
            (LeqWindow::Sliding { seconds: 10.0 }, Weighting::C, "LCeq,10s"),
        ];
        for (window, weighting, want) in cases {
            let spec = LeqSpec {
                id: "x".into(),
                label: String::new(),
                weighting,
                window,
            };
            assert_eq!(spec.derived_label(), want);
        }
    }

    #[test]
    fn a_user_label_wins_over_the_derived_one() {
        let spec = LeqSpec {
            id: "x".into(),
            label: "Front of house".into(),
            weighting: Weighting::A,
            window: LeqWindow::Elapsed,
        };
        assert_eq!(spec.display_label(), "Front of house");
    }
}
