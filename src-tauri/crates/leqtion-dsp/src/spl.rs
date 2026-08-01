//! Time-weighted sound pressure level.
//!
//! Everything in this module works in **mean square** and only converts to dB at
//! the edge. Averaging, decay and min/max all have to happen in the energy
//! domain: averaging decibels averages logarithms, which is not a level and is
//! not what any standard means.
//!
//! The dBFS convention matches [`crate::bands`] — a full-scale sine reads
//! 0 dBFS, so mean square is offset by +3.0103 dB. Calibration then adds a
//! single offset to reach dB SPL, and because the same offset applies to band
//! levels the RTA and the SPL readout can never disagree.

use serde::{Deserialize, Serialize};

use crate::bands::FULL_SCALE_SINE_OFFSET_DB;

/// The floor used in place of `-inf` for a digitally silent signal.
///
/// Real silence is a genuine possibility here (a muted input, a disconnected
/// interface) and `-inf` propagates through every average and chart it touches.
/// −200 dBFS is far below any converter's noise floor, so nothing real is ever
/// clamped by it.
pub const SILENCE_DBFS: f64 = -200.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TimeWeighting {
    /// τ = 125 ms.
    Fast,
    /// τ = 1 s.
    Slow,
    /// 35 ms rise, 1.5 s decay — the asymmetric one.
    Impulse,
}

impl TimeWeighting {
    pub const ALL: [TimeWeighting; 3] = [
        TimeWeighting::Fast,
        TimeWeighting::Slow,
        TimeWeighting::Impulse,
    ];

    pub fn label(self) -> &'static str {
        match self {
            TimeWeighting::Fast => "F",
            TimeWeighting::Slow => "S",
            TimeWeighting::Impulse => "I",
        }
    }

    /// Rise time constant, seconds.
    pub fn tau(self) -> f64 {
        match self {
            TimeWeighting::Fast => 0.125,
            TimeWeighting::Slow => 1.0,
            TimeWeighting::Impulse => 0.035,
        }
    }
}

/// Decay time constant of the Impulse detector's hold stage, seconds.
///
/// 1.5 s on a power quantity is a decay of `10/(1.5·ln 10)` = 2.895 dB/s, which
/// is the 2.9 dB/s the standard specifies. The number to change if that ever
/// looks wrong is this one, not the 2.9.
const IMPULSE_DECAY_TAU: f64 = 1.5;

/// Exponential mean-square detector: the level part of a sound level meter.
#[derive(Debug, Clone)]
pub struct LevelDetector {
    weighting: TimeWeighting,
    sample_rate: f64,
    /// Per-sample one-pole coefficient for the rise stage.
    alpha: f64,
    /// Per-sample decay factor for the Impulse hold stage.
    decay: f64,
    ms: f64,
    hold: f64,
}

impl LevelDetector {
    pub fn new(weighting: TimeWeighting, sample_rate: f64) -> Self {
        let dt = 1.0 / sample_rate;
        LevelDetector {
            weighting,
            sample_rate,
            alpha: 1.0 - (-dt / weighting.tau()).exp(),
            decay: (-dt / IMPULSE_DECAY_TAU).exp(),
            ms: 0.0,
            hold: 0.0,
        }
    }

    pub fn weighting(&self) -> TimeWeighting {
        self.weighting
    }

    pub fn sample_rate(&self) -> f64 {
        self.sample_rate
    }

    /// Feed a block. Returns the detector's mean square after the block.
    pub fn push(&mut self, samples: &[f32]) -> f64 {
        for &x in samples {
            let x = x as f64;
            self.ms += self.alpha * (x * x - self.ms);

            if matches!(self.weighting, TimeWeighting::Impulse) {
                // Rise instantly to the fast average, fall at 2.9 dB/s.
                self.hold *= self.decay;
                if self.ms > self.hold {
                    self.hold = self.ms;
                }
            }
        }
        self.mean_square()
    }

    pub fn mean_square(&self) -> f64 {
        match self.weighting {
            TimeWeighting::Impulse => self.hold,
            _ => self.ms,
        }
    }

    /// Current level, dBFS.
    pub fn level_dbfs(&self) -> f64 {
        mean_square_to_dbfs(self.mean_square())
    }

    pub fn reset(&mut self) {
        self.ms = 0.0;
        self.hold = 0.0;
    }
}

/// Mean square → dBFS, full-scale-sine referenced.
pub fn mean_square_to_dbfs(ms: f64) -> f64 {
    if ms <= 0.0 {
        return SILENCE_DBFS;
    }
    (10.0 * ms.log10() + FULL_SCALE_SINE_OFFSET_DB).max(SILENCE_DBFS)
}

/// Linear amplitude → dBFS, for peaks. A ±1.0 peak is 0 dBFS.
pub fn amplitude_to_dbfs(a: f64) -> f64 {
    if a <= 0.0 {
        return SILENCE_DBFS;
    }
    (20.0 * a.log10()).max(SILENCE_DBFS)
}

/// Running peak and true-peak-ish sample peak over a measurement.
///
/// This is a *sample* peak, not an inter-sample true peak — reconstructing the
/// analogue waveform between samples would need oversampling, and for a
/// measurement microphone the difference is far below the uncertainty of the
/// calibration. It is called `sample_peak` rather than `true_peak` so nobody
/// later assumes otherwise.
#[derive(Debug, Clone, Default)]
pub struct PeakTracker {
    sample_peak: f64,
    clipped: bool,
}

impl PeakTracker {
    pub fn push(&mut self, samples: &[f32]) -> f64 {
        let mut block_peak = 0.0f64;
        for &x in samples {
            let a = (x as f64).abs();
            if a > block_peak {
                block_peak = a;
            }
        }
        if block_peak > self.sample_peak {
            self.sample_peak = block_peak;
        }
        // A converter that has run out of headroom reports exactly ±1.0 for
        // runs of samples. Anything at or above full scale is worth flagging,
        // because every level downstream of it is now a lower bound.
        if block_peak >= 1.0 {
            self.clipped = true;
        }
        block_peak
    }

    pub fn peak(&self) -> f64 {
        self.sample_peak
    }

    pub fn peak_dbfs(&self) -> f64 {
        amplitude_to_dbfs(self.sample_peak)
    }

    pub fn clipped(&self) -> bool {
        self.clipped
    }

    pub fn reset(&mut self) {
        self.sample_peak = 0.0;
        self.clipped = false;
    }
}

/// Running maximum and minimum of a level, held in the energy domain.
#[derive(Debug, Clone)]
pub struct MinMax {
    max_ms: f64,
    min_ms: f64,
    seen: bool,
}

impl Default for MinMax {
    fn default() -> Self {
        MinMax {
            max_ms: 0.0,
            min_ms: f64::INFINITY,
            seen: false,
        }
    }
}

impl MinMax {
    pub fn push(&mut self, ms: f64) {
        if !ms.is_finite() {
            return;
        }
        self.seen = true;
        if ms > self.max_ms {
            self.max_ms = ms;
        }
        if ms < self.min_ms {
            self.min_ms = ms;
        }
    }

    pub fn max_dbfs(&self) -> f64 {
        if !self.seen {
            return SILENCE_DBFS;
        }
        mean_square_to_dbfs(self.max_ms)
    }

    pub fn min_dbfs(&self) -> f64 {
        if !self.seen {
            return SILENCE_DBFS;
        }
        mean_square_to_dbfs(self.min_ms)
    }

    pub fn reset(&mut self) {
        *self = MinMax::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(rate: f64, hz: f64, amplitude: f64, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| (amplitude * (2.0 * std::f64::consts::PI * hz * i as f64 / rate).sin()) as f32)
            .collect()
    }

    #[test]
    fn full_scale_sine_reads_zero_dbfs() {
        let rate = 48000.0;
        let mut d = LevelDetector::new(TimeWeighting::Fast, rate);
        d.push(&sine(rate, 1000.0, 1.0, 48000));
        assert!(
            d.level_dbfs().abs() < 0.01,
            "full-scale sine read {:.3} dBFS",
            d.level_dbfs()
        );
    }

    #[test]
    fn halving_amplitude_costs_six_decibels() {
        let rate = 48000.0;
        let mut a = LevelDetector::new(TimeWeighting::Slow, rate);
        let mut b = LevelDetector::new(TimeWeighting::Slow, rate);
        a.push(&sine(rate, 1000.0, 1.0, 48000 * 5));
        b.push(&sine(rate, 1000.0, 0.5, 48000 * 5));
        let diff = a.level_dbfs() - b.level_dbfs();
        assert!((diff - 6.0206).abs() < 0.01, "difference was {diff:.4} dB");
    }

    /// The defining property of Fast weighting: after one time constant of a
    /// step, the detector has covered 1 - 1/e of the energy step, which is
    /// -1.88 dB short of the final level.
    #[test]
    fn fast_reaches_the_expected_level_after_one_time_constant() {
        let rate = 48000.0;
        let mut d = LevelDetector::new(TimeWeighting::Fast, rate);
        d.push(&sine(rate, 1000.0, 1.0, (0.125 * rate) as usize));
        let expected = 10.0 * (1.0 - (-1.0f64).exp()).log10();
        assert!(
            (d.level_dbfs() - expected).abs() < 0.1,
            "after one τ: {:.3} dB, expected about {expected:.3}",
            d.level_dbfs()
        );
    }

    #[test]
    fn slow_is_slower_than_fast() {
        let rate = 48000.0;
        let burst = sine(rate, 1000.0, 1.0, (0.2 * rate) as usize);
        let mut fast = LevelDetector::new(TimeWeighting::Fast, rate);
        let mut slow = LevelDetector::new(TimeWeighting::Slow, rate);
        fast.push(&burst);
        slow.push(&burst);
        assert!(
            fast.level_dbfs() > slow.level_dbfs() + 3.0,
            "fast {:.2} vs slow {:.2}",
            fast.level_dbfs(),
            slow.level_dbfs()
        );
    }

    /// Impulse decays at 2.9 dB/s once the signal stops. This is the number the
    /// standard states, so it is worth measuring rather than assuming.
    #[test]
    fn impulse_decays_at_2_9_db_per_second() {
        let rate = 48000.0;
        let mut d = LevelDetector::new(TimeWeighting::Impulse, rate);
        d.push(&sine(rate, 1000.0, 1.0, rate as usize));
        let start = d.level_dbfs();
        d.push(&vec![0.0f32; rate as usize]);
        let after_one_second = d.level_dbfs();
        let drop = start - after_one_second;
        assert!(
            (drop - 2.895).abs() < 0.05,
            "impulse decayed {drop:.3} dB in one second"
        );
    }

    #[test]
    fn impulse_rises_faster_than_fast() {
        let rate = 48000.0;
        let click = sine(rate, 1000.0, 1.0, (0.01 * rate) as usize);
        let mut imp = LevelDetector::new(TimeWeighting::Impulse, rate);
        let mut fast = LevelDetector::new(TimeWeighting::Fast, rate);
        imp.push(&click);
        fast.push(&click);
        assert!(
            imp.level_dbfs() > fast.level_dbfs(),
            "impulse {:.2} should exceed fast {:.2} on a 10 ms click",
            imp.level_dbfs(),
            fast.level_dbfs()
        );
    }

    #[test]
    fn silence_does_not_produce_infinities() {
        let mut d = LevelDetector::new(TimeWeighting::Fast, 48000.0);
        d.push(&vec![0.0f32; 4800]);
        assert_eq!(d.level_dbfs(), SILENCE_DBFS);
        assert!(d.level_dbfs().is_finite());
        assert_eq!(amplitude_to_dbfs(0.0), SILENCE_DBFS);
        assert_eq!(mean_square_to_dbfs(0.0), SILENCE_DBFS);
    }

    #[test]
    fn peak_tracker_flags_clipping_only_at_full_scale() {
        let mut p = PeakTracker::default();
        p.push(&[0.5, -0.99, 0.2]);
        assert!(!p.clipped());
        assert!((p.peak_dbfs() - (-0.0873)).abs() < 0.01);
        p.push(&[1.0]);
        assert!(p.clipped());
        assert!(p.peak_dbfs().abs() < 1e-9);
    }

    #[test]
    fn minmax_holds_energy_not_decibels() {
        let mut m = MinMax::default();
        m.push(0.5);
        m.push(0.125);
        m.push(0.25);
        assert!((m.max_dbfs() - mean_square_to_dbfs(0.5)).abs() < 1e-12);
        assert!((m.min_dbfs() - mean_square_to_dbfs(0.125)).abs() < 1e-12);
    }

    #[test]
    fn empty_minmax_is_silent_not_infinite() {
        let m = MinMax::default();
        assert_eq!(m.max_dbfs(), SILENCE_DBFS);
        assert_eq!(m.min_dbfs(), SILENCE_DBFS);
    }
}
