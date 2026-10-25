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
//! Three window shapes, because all three are asked for:
//!
//! - [`LeqWindow::Sliding`] — the last N seconds, continuously. This is what a
//!   "short-term LEQ" or a 5-minute rolling limit means, and it is what a
//!   venue's noise limiter is usually watching.
//! - [`LeqWindow::Elapsed`] — everything since the last reset. The show LEQ.
//! - [`LeqWindow::Periodic`] — consecutive fixed periods, each its own LEQ:
//!   LAeq,15min the way a licence usually means it, one figure per quarter
//!   hour. Completed periods are kept, so they can be drawn as bars.
//!
//! ## Periods are on the clock when the clock is known
//!
//! A licence's quarter hours start at :00, :15, :30 and :45, not at whenever
//! someone pressed Reset. This module has no clock (crate rule), so the caller
//! hands an accumulator the time of day at which it started, and the period
//! boundaries are placed from that. The period running when the accumulator
//! started is therefore usually short, and is marked incomplete rather than
//! passed off as a full one: a 6-minute LAeq drawn as a 15-minute bar would
//! be compared against a 15-minute limit by whoever reads the chart. With no
//! time of day, periods start at the reset.
//!
//! The time base is the audio, not the wall: a period ends when a period's
//! worth of samples has arrived. Against the wall clock that drifts by the
//! converter's error — tens of parts per million, a few seconds a day — and
//! the boundaries are put back on the clock at every reset.
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
    /// Back-to-back periods of `seconds`, each its own LEQ.
    Periodic { seconds: f64 },
}

impl LeqWindow {
    pub fn label(self) -> String {
        match self {
            LeqWindow::Elapsed => "elapsed".to_string(),
            LeqWindow::Sliding { seconds } => format_duration(seconds),
            LeqWindow::Periodic { seconds } => format!("every {}", format_duration(seconds)),
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
            // Distinguished from the sliding window of the same length, because
            // the two give different numbers and a log column has to say which.
            LeqWindow::Periodic { seconds } => format!(
                "L{}eq,{} periods",
                self.weighting.label(),
                compact_duration(seconds)
            ),
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

/// Completed periods kept per accumulator. A week of quarter hours, or a
/// week of hours; at one-minute periods, about a week too.
pub const MAX_PERIODS: usize = 10_080;

/// Shortest period accepted. Anything shorter is a sliding LEQ's job, and a
/// bar per second would only be a level history drawn badly.
pub const MIN_PERIOD_SECONDS: f64 = 10.0;

const SECONDS_PER_DAY: f64 = 86_400.0;

#[derive(Debug, Clone, Copy)]
struct Slice {
    /// Mean square × duration, i.e. energy.
    energy: f64,
    seconds: f64,
}

/// One finished period of a periodic LEQ.
#[derive(Debug, Clone, Copy)]
struct PeriodRecord {
    /// Seconds after the reset at which it began.
    start: f64,
    seconds: f64,
    energy: f64,
    /// Energy and time since the reset, as of the end of this period.
    total_energy: f64,
    total_seconds: f64,
    /// Whether it ran from one boundary to the next.
    complete: bool,
}

/// One period, as sent to the UI.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LeqPeriod {
    /// Seconds after the reset at which the period began.
    pub start_seconds: f64,
    /// How much of it has been measured.
    pub seconds: f64,
    /// The period's LEQ. dBFS from the accumulator; the engine adds the
    /// calibration before it leaves.
    pub value: f64,
    /// The LEQ of everything since the reset, as of the end of this period.
    pub total: f64,
    /// Ran from one boundary to the next. A period that started part-way —
    /// because the measurement did — is not comparable with a limit set for
    /// the full length.
    pub complete: bool,
    /// The period still being measured.
    pub current: bool,
    /// Time of day it began, `HH:MM` (or `HH:MM:SS` off the minute), when the
    /// accumulator was given a time of day.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clock: Option<String>,
}

/// Every period of one periodic LEQ, as sent to the UI.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LeqPeriods {
    pub id: String,
    pub label: String,
    pub weighting: Weighting,
    pub period_seconds: f64,
    /// True when `value` and `total` are dB SPL rather than dBFS.
    pub calibrated: bool,
    /// Oldest first; the period in progress is last.
    pub periods: Vec<LeqPeriod>,
}

/// The period schedule and the finished periods. Only exists for a periodic
/// window.
#[derive(Debug, Clone)]
struct Periods {
    length: f64,
    start: f64,
    end: f64,
    /// The running period began on a boundary.
    full: bool,
    done: VecDeque<PeriodRecord>,
    total_energy: f64,
    total_seconds: f64,
}

impl Periods {
    /// The schedule for an accumulator that starts now, at `origin` seconds
    /// into the day if that is known.
    fn new(length: f64, origin: Option<f64>) -> Self {
        let length = length.max(MIN_PERIOD_SECONDS);
        let (end, full) = match origin {
            Some(o) => {
                let into = o.rem_euclid(SECONDS_PER_DAY).rem_euclid(length);
                let to_boundary = length - into;
                // Within a millisecond of a boundary is on it; anything else
                // is a period already under way.
                if to_boundary < 1e-3 {
                    (to_boundary + length, true)
                } else {
                    (to_boundary, into < 1e-3)
                }
            }
            None => (length, true),
        };
        Periods {
            length,
            start: 0.0,
            end,
            full,
            done: VecDeque::new(),
            total_energy: 0.0,
            total_seconds: 0.0,
        }
    }
}

/// One running LEQ.
#[derive(Debug, Clone)]
pub struct LeqAccumulator {
    spec: LeqSpec,
    /// Only populated for a sliding window.
    ring: VecDeque<Slice>,
    /// For a periodic window: the current period's energy and time.
    energy: f64,
    seconds: f64,
    /// Total time observed since reset, which for a sliding window keeps
    /// running after the window is full.
    elapsed: f64,
    pops: u64,
    /// Seconds into the day at which this accumulator last started. See the
    /// module docs.
    origin: Option<f64>,
    /// An origin handed over mid-measurement, waiting for the next reset, as
    /// the time of day of this accumulator's zero.
    pending_origin: Option<Option<f64>>,
    periods: Option<Periods>,
}

impl LeqAccumulator {
    pub fn new(spec: LeqSpec) -> Self {
        let mut a = LeqAccumulator {
            spec,
            ring: VecDeque::new(),
            energy: 0.0,
            seconds: 0.0,
            elapsed: 0.0,
            pops: 0,
            origin: None,
            pending_origin: None,
            periods: None,
        };
        a.reset();
        a
    }

    /// Tell the accumulator the time of day *now*, in seconds since midnight.
    ///
    /// Takes effect at once only before anything has been measured — moving
    /// the boundaries of a period already holding energy would put that energy
    /// in a period it was not measured in. Otherwise it waits for the next
    /// reset. It is held as the time of day of this accumulator's own zero, so
    /// it still means the right thing whenever that reset comes.
    pub fn set_origin(&mut self, seconds_of_day: Option<f64>) {
        self.pending_origin = Some(seconds_of_day.map(|o| o - self.elapsed));
        if self.elapsed == 0.0 {
            self.reset();
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
        // Sliding to sliding keeps what still fits. Any other change of window
        // means the energy held so far was gathered for a different question.
        let same_shape = match (self.spec.window, spec.window) {
            (LeqWindow::Sliding { .. }, LeqWindow::Sliding { .. }) => true,
            (LeqWindow::Elapsed, LeqWindow::Elapsed) => true,
            (LeqWindow::Periodic { seconds: a }, LeqWindow::Periodic { seconds: b }) => a == b,
            _ => false,
        };
        self.spec = spec;

        if weighting_changed || !same_shape {
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
        if let Some(periods) = self.periods.as_mut() {
            Self::push_periodic(
                periods,
                &mut self.energy,
                &mut self.seconds,
                &mut self.elapsed,
                mean_square,
                seconds,
            );
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
            LeqWindow::Periodic { .. } => unreachable!("handled above"),
        }
    }

    /// Feed a block into the period schedule, splitting it at a boundary.
    ///
    /// A block that straddles a boundary is shared out in proportion to the
    /// time on each side, which assumes its mean square was steady across it.
    /// Blocks are tens of milliseconds and periods are minutes, so the most
    /// that can move is one block's worth of energy between neighbours.
    fn push_periodic(
        p: &mut Periods,
        energy: &mut f64,
        period_seconds: &mut f64,
        elapsed: &mut f64,
        mean_square: f64,
        seconds: f64,
    ) {
        let mut left = seconds;
        while left > 0.0 {
            let take = left.min((p.end - *elapsed).max(0.0));
            *energy += mean_square * take;
            *period_seconds += take;
            p.total_energy += mean_square * take;
            p.total_seconds += take;
            *elapsed += take;
            left -= take;

            if *elapsed >= p.end - 1e-9 {
                p.done.push_back(PeriodRecord {
                    start: p.start,
                    seconds: *period_seconds,
                    energy: *energy,
                    total_energy: p.total_energy,
                    total_seconds: p.total_seconds,
                    complete: p.full,
                });
                if p.done.len() > MAX_PERIODS {
                    p.done.pop_front();
                }
                *energy = 0.0;
                *period_seconds = 0.0;
                p.start = p.end;
                p.end += p.length;
                p.full = true;
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
            // How far through the current period, against its full length —
            // so a short first period never reads as full.
            LeqWindow::Periodic { .. } => match &self.periods {
                Some(p) => ((self.elapsed - (p.end - p.length)) / p.length).clamp(0.0, 1.0),
                None => 0.0,
            },
        }
    }

    pub fn reset(&mut self) {
        // Restarting now means starting later in the day than last time. Unless
        // the caller has said otherwise, carry the clock forward by what was
        // measured, so periods placed after a reset are still on the clock.
        self.origin = match self.pending_origin.take() {
            Some(o) => o,
            None => self.origin,
        }
        .map(|o| o + self.elapsed);
        self.ring.clear();
        self.energy = 0.0;
        self.seconds = 0.0;
        self.elapsed = 0.0;
        self.pops = 0;
        self.periods = match self.spec.window {
            LeqWindow::Periodic { seconds } => Some(Periods::new(seconds, self.origin)),
            _ => None,
        };
    }

    /// The last `max` periods, oldest first, with the one in progress last.
    /// Empty for any other window. Levels are dBFS.
    pub fn periods(&self, max: usize) -> Vec<LeqPeriod> {
        let Some(p) = &self.periods else {
            return Vec::new();
        };
        let level = |energy: f64, seconds: f64| {
            if seconds > 0.0 {
                mean_square_to_dbfs(energy / seconds)
            } else {
                SILENCE_DBFS
            }
        };
        let clock = |start: f64| self.origin.map(|o| clock_label(o + start));

        let max = max.max(1);
        let skip = (p.done.len() + 1).saturating_sub(max);
        let mut out: Vec<LeqPeriod> = p
            .done
            .iter()
            .skip(skip)
            .map(|r| LeqPeriod {
                start_seconds: r.start,
                seconds: r.seconds,
                value: level(r.energy, r.seconds),
                total: level(r.total_energy, r.total_seconds),
                complete: r.complete,
                current: false,
                clock: clock(r.start),
            })
            .collect();
        out.push(LeqPeriod {
            start_seconds: p.start,
            seconds: self.seconds,
            value: level(self.energy, self.seconds),
            total: level(p.total_energy, p.total_seconds),
            complete: false,
            current: true,
            clock: clock(p.start),
        });
        out
    }

    /// Length of one period, for a periodic window.
    pub fn period_seconds(&self) -> Option<f64> {
        self.periods.as_ref().map(|p| p.length)
    }
}

/// Time of day for a period label. Whole minutes are `HH:MM`; anything else
/// (a 90-second period, a reset at an odd moment) gets its seconds too, rather
/// than two bars both labelled 20:07.
fn clock_label(seconds_of_day: f64) -> String {
    let t = (seconds_of_day.rem_euclid(SECONDS_PER_DAY) + 0.5).floor() as u64 % 86_400;
    let (h, m, s) = (t / 3600, (t / 60) % 60, t % 60);
    if s == 0 {
        format!("{h:02}:{m:02}")
    } else {
        format!("{h:02}:{m:02}:{s:02}")
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

    fn periodic(seconds: f64) -> LeqSpec {
        LeqSpec {
            id: "p".into(),
            label: String::new(),
            weighting: Weighting::A,
            window: LeqWindow::Periodic { seconds },
        }
    }

    /// 20:07:30, in seconds since midnight.
    const SEVEN_PAST_EIGHT_AND_A_HALF: f64 = 20.0 * 3600.0 + 7.0 * 60.0 + 30.0;

    #[test]
    fn each_period_is_its_own_leq_and_the_total_runs_across_them() {
        let mut a = LeqAccumulator::new(periodic(60.0));
        for _ in 0..600 {
            a.push(1.0, 0.1);
        }
        for _ in 0..600 {
            a.push(0.01, 0.1);
        }
        let p = a.periods(10);
        // Two finished periods and the empty one that has just begun.
        assert_eq!(p.len(), 3, "{p:?}");
        assert!((p[0].value - mean_square_to_dbfs(1.0)).abs() < 1e-9);
        assert!((p[1].value - mean_square_to_dbfs(0.01)).abs() < 1e-9);
        assert!((p[0].total - p[0].value).abs() < 1e-9);
        // The running total is an energy mean, not a mean of the two bars.
        let total = mean_square_to_dbfs((1.0 + 0.01) / 2.0);
        assert!((p[1].total - total).abs() < 1e-9, "{} vs {total}", p[1].total);
        assert!(p[0].complete && p[1].complete);
        assert!(p[2].current && !p[2].complete);
        assert_eq!(p[2].start_seconds, 120.0);
    }

    /// A block that straddles a boundary is shared out by time, so neither
    /// period gains or loses the part that belongs to the other.
    #[test]
    fn a_block_across_a_boundary_is_split_by_time() {
        let mut a = LeqAccumulator::new(periodic(10.0));
        a.push(1.0, 9.5);
        a.push(0.0, 1.0);
        let p = a.periods(10);
        assert_eq!(p.len(), 2);
        assert!((p[0].seconds - 10.0).abs() < 1e-9);
        assert!((p[0].value - mean_square_to_dbfs(0.95)).abs() < 1e-9);
        assert!((p[1].seconds - 0.5).abs() < 1e-9);
        assert_eq!(p[1].value, SILENCE_DBFS);
    }

    /// Quarter hours start at :15, not at whenever Reset was pressed — and the
    /// stub before the first boundary is marked short rather than drawn as a
    /// full quarter hour.
    #[test]
    fn periods_land_on_the_clock_and_the_first_one_is_marked_short() {
        let mut a = LeqAccumulator::new(periodic(900.0));
        a.set_origin(Some(SEVEN_PAST_EIGHT_AND_A_HALF));
        for _ in 0..1500 {
            a.push(0.5, 1.0);
        }
        let p = a.periods(10);
        assert_eq!(p[0].clock.as_deref(), Some("20:07:30"));
        assert!((p[0].seconds - 450.0).abs() < 1e-9, "{}", p[0].seconds);
        assert!(!p[0].complete);
        assert_eq!(p[1].clock.as_deref(), Some("20:15"));
        assert!(p[1].complete);
        assert_eq!(p[2].clock.as_deref(), Some("20:30"));
        assert!(p[2].current);
    }

    #[test]
    fn starting_on_a_boundary_gives_a_full_first_period() {
        let mut a = LeqAccumulator::new(periodic(900.0));
        a.set_origin(Some(20.0 * 3600.0 + 15.0 * 60.0));
        a.push(0.5, 900.0);
        let p = a.periods(10);
        assert!(p[0].complete);
        assert_eq!(p[0].clock.as_deref(), Some("20:15"));
        assert_eq!(p[1].clock.as_deref(), Some("20:30"));
    }

    /// A period begun a few hundred microseconds early is on the boundary, not
    /// a period one sliver long.
    #[test]
    fn a_hair_before_a_boundary_counts_as_on_it() {
        let mut a = LeqAccumulator::new(periodic(900.0));
        a.set_origin(Some(20.0 * 3600.0 + 15.0 * 60.0 - 0.0005));
        a.push(0.5, 10.0);
        let p = a.periods(10);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].clock.as_deref(), Some("20:15"));
    }

    /// Fill is against the clock period, so a short first period never reads
    /// as full: at 20:07:30 a quarter hour is already half gone.
    #[test]
    fn fill_counts_the_whole_clock_period() {
        let mut a = LeqAccumulator::new(periodic(900.0));
        a.set_origin(Some(SEVEN_PAST_EIGHT_AND_A_HALF));
        a.push(0.5, 0.001);
        assert!((a.fill() - 0.5).abs() < 1e-3, "{}", a.fill());
        a.push(0.5, 225.0);
        assert!((a.fill() - 0.75).abs() < 1e-3, "{}", a.fill());
    }

    #[test]
    fn a_new_period_length_restarts_from_now_on_the_clock() {
        let mut a = LeqAccumulator::new(periodic(900.0));
        a.set_origin(Some(SEVEN_PAST_EIGHT_AND_A_HALF));
        a.push(0.5, 150.0);
        a.reconfigure(periodic(300.0));
        assert_eq!(a.elapsed_seconds(), 0.0);
        a.push(0.5, 1.0);
        let p = a.periods(10);
        // Restarted at 20:10:00, which is on a five-minute boundary.
        assert_eq!(p[0].clock.as_deref(), Some("20:10"));
        assert!(p[0].current);
    }

    /// An origin handed over after measuring has begun would move energy into
    /// a period it was not measured in, so it waits for the next reset.
    #[test]
    fn an_origin_arriving_mid_period_waits_for_the_reset() {
        let mut a = LeqAccumulator::new(periodic(60.0));
        a.push(0.5, 30.0);
        a.set_origin(Some(SEVEN_PAST_EIGHT_AND_A_HALF));
        assert!(a.periods(10)[0].clock.is_none());
        a.reset();
        a.push(0.5, 1.0);
        assert_eq!(a.periods(10)[0].clock.as_deref(), Some("20:07:30"));
    }

    #[test]
    fn the_periods_kept_are_capped() {
        let mut a = LeqAccumulator::new(periodic(MIN_PERIOD_SECONDS));
        for _ in 0..(MAX_PERIODS + 5) {
            a.push(0.5, MIN_PERIOD_SECONDS);
        }
        let p = a.periods(usize::MAX);
        assert_eq!(p.len(), MAX_PERIODS + 1);
        assert!(p.last().unwrap().current);
        let only_three = a.periods(3);
        assert_eq!(only_three.len(), 3);
        assert!(only_three[2].current);
    }

    #[test]
    fn a_period_shorter_than_the_minimum_is_lengthened() {
        let a = LeqAccumulator::new(periodic(1.0));
        assert_eq!(a.period_seconds(), Some(MIN_PERIOD_SECONDS));
        assert!(LeqAccumulator::new(sliding(1.0)).period_seconds().is_none());
    }

    /// The live readout of a periodic LEQ is the period in progress.
    #[test]
    fn the_readout_is_the_period_in_progress() {
        let mut a = LeqAccumulator::new(periodic(10.0));
        a.push(1.0, 10.0);
        a.push(0.01, 5.0);
        assert!((a.leq_dbfs() - mean_square_to_dbfs(0.01)).abs() < 1e-9);
        assert!((a.integrated_seconds() - 5.0).abs() < 1e-9);
        assert!((a.elapsed_seconds() - 15.0).abs() < 1e-9);
    }

    #[test]
    fn a_periodic_leq_is_named_so_it_cannot_pass_for_a_sliding_one() {
        assert_eq!(periodic(900.0).derived_label(), "LAeq,15min periods");
        assert_eq!(LeqWindow::Periodic { seconds: 900.0 }.label(), "every 15 min");
        let v = serde_json::to_value(LeqWindow::Periodic { seconds: 900.0 }).unwrap();
        assert_eq!(v, serde_json::json!({ "kind": "periodic", "seconds": 900.0 }));
    }

    #[test]
    fn period_wire_format_is_camel_case() {
        let mut a = LeqAccumulator::new(periodic(60.0));
        a.set_origin(Some(0.0));
        a.push(0.5, 61.0);
        let v = serde_json::to_value(&a.periods(10)[0]).unwrap();
        for key in ["startSeconds", "seconds", "value", "total", "complete", "current", "clock"] {
            assert!(v.get(key).is_some(), "{key} missing from {v}");
        }
        assert_eq!(v["clock"], "00:00");
    }
}
