//! The RTA: overlapped transforms integrated into fractional-octave bands.
//!
//! Samples go into a ring; every `hop` samples a windowed transform runs and its
//! bin powers are integrated into bands. Band powers are then averaged **in the
//! power domain** — averaging dB would bias every answer low, and by an amount
//! that depends on how much the signal moves.
//!
//! Nothing here knows about weighting. The RTA shows what is there; a weighting
//! curve is applied at the display, and the weighted *levels* come from
//! [`crate::spl`] and [`crate::leq`], which filter in the time domain.

use realfft::num_complex::Complex;
use realfft::{RealFftPlanner, RealToComplex};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::bands::{build_band_plan, integrate_bands, power_to_db, BandPlan, Fraction};
use crate::window::{Window, WindowKind};

/// Transform sizes offered. The lower bound keeps a 1/3-octave display honest
/// at 20 Hz; the upper bound is where a 1/48-octave display stops being an
/// interpolation at the bottom of the range.
pub const FFT_SIZES: [usize; 6] = [2048, 4096, 8192, 16384, 32768, 65536];

/// Exponential averaging time constants, in seconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Averaging {
    /// τ = 125 ms.
    Fast,
    /// τ = 1 s.
    Slow,
    /// τ = 4 s.
    Long,
    /// A linear running mean over every frame since the last reset — the one to
    /// use for measuring a room with pink noise, where the answer should stop
    /// moving.
    Infinite,
}

impl Averaging {
    pub const ALL: [Averaging; 4] = [
        Averaging::Fast,
        Averaging::Slow,
        Averaging::Long,
        Averaging::Infinite,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Averaging::Fast => "Fast",
            Averaging::Slow => "Slow",
            Averaging::Long => "Long",
            Averaging::Infinite => "Infinite",
        }
    }

    pub fn tau(self) -> Option<f64> {
        match self {
            Averaging::Fast => Some(0.125),
            Averaging::Slow => Some(1.0),
            Averaging::Long => Some(4.0),
            Averaging::Infinite => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpectrumConfig {
    pub fraction: Fraction,
    pub fft_size: usize,
    pub window: WindowKind,
    /// Frame advance as a fraction of the transform: 0.25 = 75% overlap.
    pub hop_fraction: f64,
    pub averaging: Averaging,
    pub peak_hold: bool,
}

impl Default for SpectrumConfig {
    fn default() -> Self {
        SpectrumConfig {
            fraction: Fraction::Twelfth,
            fft_size: 16384,
            window: WindowKind::Hann,
            hop_fraction: 0.5,
            averaging: Averaging::Slow,
            peak_hold: false,
        }
    }
}

impl SpectrumConfig {
    fn sanitised(mut self) -> Self {
        if !FFT_SIZES.contains(&self.fft_size) {
            self.fft_size = 16384;
        }
        if !(0.05..=1.0).contains(&self.hop_fraction) {
            self.hop_fraction = 0.5;
        }
        self
    }

    fn hop(&self) -> usize {
        ((self.fft_size as f64 * self.hop_fraction).round() as usize).clamp(1, self.fft_size)
    }
}

pub struct SpectrumAnalyser {
    config: SpectrumConfig,
    sample_rate: f64,
    plan: BandPlan,
    window: Window,
    fft: Arc<dyn RealToComplex<f64>>,

    /// Circular sample history, `fft_size` long.
    ring: Vec<f64>,
    write: usize,
    /// Samples received since the last transform.
    since_hop: usize,
    hop: usize,

    scratch_time: Vec<f64>,
    scratch_freq: Vec<Complex<f64>>,
    power: Vec<f64>,

    band_power: Vec<f64>,
    averaged: Vec<f64>,
    peaks_db: Vec<f32>,
    bands_db: Vec<f32>,
    dominant_hz: Option<f64>,
    frames: u64,
    /// Total transforms run since construction — the UI shows it so a very long
    /// transform at a low overlap does not look like a hung display.
    transforms: u64,
}

impl SpectrumAnalyser {
    pub fn new(config: SpectrumConfig, sample_rate: f64) -> Self {
        let config = config.sanitised();
        let window = Window::new(config.window, config.fft_size);
        let plan = build_band_plan(config.fraction, config.fft_size, sample_rate, window.enbw());

        let mut planner = RealFftPlanner::<f64>::new();
        let fft = planner.plan_fft_forward(config.fft_size);
        let scratch_freq = fft.make_output_vec();
        let bins = scratch_freq.len();
        let n_bands = plan.bands.len();

        SpectrumAnalyser {
            hop: config.hop(),
            ring: vec![0.0; config.fft_size],
            write: 0,
            since_hop: 0,
            scratch_time: vec![0.0; config.fft_size],
            scratch_freq,
            power: vec![0.0; bins],
            band_power: vec![0.0; n_bands],
            averaged: vec![0.0; n_bands],
            peaks_db: vec![f32::NEG_INFINITY; n_bands],
            bands_db: vec![-200.0; n_bands],
            dominant_hz: None,
            frames: 0,
            transforms: 0,
            config,
            sample_rate,
            plan,
            window,
            fft,
        }
    }

    pub fn config(&self) -> SpectrumConfig {
        self.config
    }

    pub fn plan(&self) -> &BandPlan {
        &self.plan
    }

    pub fn sample_rate(&self) -> f64 {
        self.sample_rate
    }

    /// Band levels in dBFS. One per `plan().bands`.
    pub fn bands_db(&self) -> &[f32] {
        &self.bands_db
    }

    /// Held peaks in dBFS, or `-inf` where nothing has been held yet.
    pub fn peaks_db(&self) -> &[f32] {
        &self.peaks_db
    }

    pub fn transforms(&self) -> u64 {
        self.transforms
    }

    /// Frequency of the strongest component in the most recent transform.
    ///
    /// `None` until a transform has run, or when the spectrum is empty enough
    /// that naming a peak would be inventing one. Interpolated across the three
    /// bins around the maximum, so the answer is not quantised to the bin
    /// spacing — at a 16384-point transform and 48 kHz that would be 2.9 Hz
    /// steps, which is too coarse to tell a 1 kHz calibrator from a mis-clocked
    /// one.
    pub fn dominant_hz(&self) -> Option<f64> {
        self.dominant_hz
    }

    /// Seconds between transforms at the current settings.
    pub fn hop_seconds(&self) -> f64 {
        self.hop as f64 / self.sample_rate
    }

    /// Apply new settings.
    ///
    /// Anything that changes the transform or the band table rebuilds the
    /// analyser: the averaged powers refer to a different set of bands and
    /// cannot be carried across. Changing only the averaging or the peak-hold
    /// flag keeps the running average, because those are the two a user adjusts
    /// while watching a measurement they do not want to restart.
    pub fn reconfigure(&mut self, config: SpectrumConfig, sample_rate: f64) {
        let config = config.sanitised();
        let structural = config.fraction != self.config.fraction
            || config.fft_size != self.config.fft_size
            || config.window != self.config.window
            || (config.hop_fraction - self.config.hop_fraction).abs() > f64::EPSILON
            || (sample_rate - self.sample_rate).abs() > f64::EPSILON;

        if structural {
            *self = SpectrumAnalyser::new(config, sample_rate);
            return;
        }

        if config.averaging != self.config.averaging {
            self.frames = 0;
        }
        if !config.peak_hold && self.config.peak_hold {
            self.reset_peaks();
        }
        self.config = config;
    }

    /// Feed samples. Returns true if at least one new transform completed, i.e.
    /// if `bands_db()` has changed.
    pub fn push(&mut self, samples: &[f32]) -> bool {
        let mut produced = false;
        for &x in samples {
            self.ring[self.write] = x as f64;
            self.write = (self.write + 1) % self.ring.len();
            self.since_hop += 1;
            if self.since_hop >= self.hop {
                self.since_hop = 0;
                self.transform();
                produced = true;
            }
        }
        produced
    }

    fn transform(&mut self) {
        let n = self.ring.len();
        // Unwrap the ring oldest-first and window it in the same pass.
        for i in 0..n {
            let idx = (self.write + i) % n;
            self.scratch_time[i] = self.ring[idx] * self.window.samples[i];
        }

        if self
            .fft
            .process(&mut self.scratch_time, &mut self.scratch_freq)
            .is_err()
        {
            // Only possible on a length mismatch, which cannot happen here
            // because both buffers come from the same plan. Leaving the last
            // frame on screen beats panicking on an audio thread.
            return;
        }

        // Bin power, normalised so that summing every bin recovers the mean
        // square of the input.
        //
        // Three separate factors, and leaving any one out is a silent gain
        // error rather than a crash:
        //
        //  - `1/S2` — normalise by the window's sum of *squares*, not its sum.
        //    That is what makes the estimate correct for noise, which is what
        //    summing bins into an octave band assumes. (Normalising by S1²
        //    would be the amplitude-correct choice for a discrete tone, and
        //    would read every band of noise wrong.)
        //  - `1/N` — an unwindowed transform is unnormalised: `E|X[k]|²` is
        //    proportional to N. Without this every level is out by 10·log10(N),
        //    which at a 16384-point transform is 42 dB.
        //  - `×2` — fold the negative-frequency half onto the positive one, for
        //    every bin except DC and Nyquist, which have no twin.
        //
        // A tone still reads correctly once its band is summed: the window
        // spreads it over roughly ENBW bins, and summing them recovers the
        // power that a single-bin reading would understate.
        let scale = 2.0 / (n as f64 * self.window.s2);
        let last = self.power.len() - 1;
        for (k, c) in self.scratch_freq.iter().enumerate() {
            let p = c.norm_sqr() * scale;
            self.power[k] = if k == 0 || k == last { p * 0.5 } else { p };
        }

        integrate_bands(&self.plan, &self.power, &mut self.band_power);
        self.dominant_hz = self.find_dominant();

        self.frames += 1;
        self.transforms += 1;

        match self.config.averaging.tau() {
            None => {
                // Linear running mean: each frame carries 1/n of the answer.
                let inv = 1.0 / self.frames as f64;
                for (avg, &p) in self.averaged.iter_mut().zip(self.band_power.iter()) {
                    *avg += (p - *avg) * inv;
                }
            }
            Some(tau) => {
                let alpha = if self.frames == 1 {
                    // Seed with the first frame rather than ramping up from
                    // silence, which otherwise reads 20 dB low for a second.
                    1.0
                } else {
                    1.0 - (-self.hop_seconds() / tau).exp()
                };
                for (avg, &p) in self.averaged.iter_mut().zip(self.band_power.iter()) {
                    *avg += (p - *avg) * alpha;
                }
            }
        }

        for i in 0..self.averaged.len() {
            let db = power_to_db(self.averaged[i]) as f32;
            self.bands_db[i] = db;
            if self.config.peak_hold && db > self.peaks_db[i] {
                self.peaks_db[i] = db;
            }
        }
    }

    /// Locate the strongest spectral component, interpolated.
    ///
    /// DC and the first bin are skipped: a converter with any DC offset puts a
    /// large value in bin 0, and reporting "0 Hz" as the dominant tone would be
    /// both useless and, during calibration, actively misleading.
    fn find_dominant(&self) -> Option<f64> {
        let last = self.power.len().saturating_sub(1);
        if last < 4 {
            return None;
        }
        // Start at the bottom of the measurement range, and never below bin 3.
        // A converter with any DC offset puts a large value in bin 0, and the
        // window smears it into bins 1 and 2 — enough that a real 1 kHz
        // calibrator loses to it. Reporting "2.9 Hz" as the dominant tone would
        // be useless in general and actively misleading during calibration.
        let first = ((crate::bands::F_MIN / self.plan.bin_hz).ceil() as usize).max(3);
        if first >= last {
            return None;
        }

        let mut best = first;
        for k in first..last {
            if self.power[k] > self.power[best] {
                best = k;
            }
        }
        // Nothing worth naming.
        if self.power[best] <= 1e-24 {
            return None;
        }

        // Parabolic interpolation on the log magnitudes of the three bins
        // around the peak. Logs rather than powers because the main lobe of a
        // windowed sinusoid is closer to a parabola in dB than in linear power,
        // which is what makes this accurate to a small fraction of a bin.
        let hz = if best > 0 && best < last {
            let l = self.power[best - 1].max(1e-30).ln();
            let c = self.power[best].max(1e-30).ln();
            let r = self.power[best + 1].max(1e-30).ln();
            let denom = l - 2.0 * c + r;
            let delta = if denom.abs() < 1e-18 {
                0.0
            } else {
                (0.5 * (l - r) / denom).clamp(-0.5, 0.5)
            };
            (best as f64 + delta) * self.plan.bin_hz
        } else {
            best as f64 * self.plan.bin_hz
        };
        Some(hz)
    }

    /// Restart the average without touching the sample history.
    pub fn reset_average(&mut self) {
        self.frames = 0;
        for v in &mut self.averaged {
            *v = 0.0;
        }
    }

    pub fn reset_peaks(&mut self) {
        for v in &mut self.peaks_db {
            *v = f32::NEG_INFINITY;
        }
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

    fn config(fraction: Fraction, fft_size: usize) -> SpectrumConfig {
        SpectrumConfig {
            fraction,
            fft_size,
            window: WindowKind::Hann,
            hop_fraction: 0.5,
            averaging: Averaging::Infinite,
            peak_hold: false,
        }
    }

    /// A full-scale sine must land in the right band at 0 dBFS. This single
    /// test covers the window normalisation, the power scaling, the band
    /// integration and the dB convention at once — if any of them is wrong the
    /// number moves.
    #[test]
    fn full_scale_sine_reads_zero_db_in_its_own_band() {
        let rate = 48000.0;
        let mut a = SpectrumAnalyser::new(config(Fraction::Third, 16384), rate);
        a.push(&sine(rate, 1000.0, 1.0, 16384 * 6));

        let idx = a
            .plan()
            .bands
            .iter()
            .position(|b| b.flo <= 1000.0 && 1000.0 < b.fhi)
            .expect("no band contains 1 kHz");
        let level = a.bands_db()[idx];
        assert!(
            (level as f64).abs() < 0.2,
            "1 kHz band read {level:.3} dBFS, expected 0"
        );
    }

    #[test]
    fn energy_lands_in_the_band_that_contains_the_tone() {
        let rate = 48000.0;
        let mut a = SpectrumAnalyser::new(config(Fraction::Third, 16384), rate);
        a.push(&sine(rate, 1000.0, 1.0, 16384 * 6));

        let loudest = a
            .bands_db()
            .iter()
            .enumerate()
            .max_by(|x, y| x.1.partial_cmp(y.1).unwrap())
            .map(|(i, _)| i)
            .unwrap();
        let b = &a.plan().bands[loudest];
        assert!(
            b.flo <= 1000.0 && 1000.0 < b.fhi,
            "loudest band was {} ({:.0}-{:.0} Hz)",
            b.label,
            b.flo,
            b.fhi
        );
        // Neighbours must be well down — this is the leakage check.
        assert!(a.bands_db()[loudest - 2] < a.bands_db()[loudest] - 40.0);
        assert!(a.bands_db()[loudest + 2] < a.bands_db()[loudest] - 40.0);
    }

    /// Halving the amplitude must cost exactly 6 dB in the band.
    #[test]
    fn band_levels_scale_with_amplitude() {
        let rate = 48000.0;
        let read = |amp: f64| {
            let mut a = SpectrumAnalyser::new(config(Fraction::Third, 16384), rate);
            a.push(&sine(rate, 1000.0, amp, 16384 * 6));
            let idx = a
                .plan()
                .bands
                .iter()
                .position(|b| b.flo <= 1000.0 && 1000.0 < b.fhi)
                .unwrap();
            a.bands_db()[idx] as f64
        };
        let diff = read(1.0) - read(0.5);
        assert!((diff - 6.0206).abs() < 0.05, "difference was {diff:.4} dB");
    }

    /// White noise has equal power per hertz, so a 1/3-octave RTA should tilt
    /// upward at 1 dB per third-octave — the classic sanity check that a band
    /// integrator is summing rather than averaging bins.
    #[test]
    fn white_noise_rises_one_db_per_third_octave() {
        let rate = 48000.0;
        let mut a = SpectrumAnalyser::new(config(Fraction::Third, 8192), rate);

        let mut state = 987_654_321u32;
        let noise: Vec<f32> = (0..8192 * 40)
            .map(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (state >> 8) as f32 / 8_388_608.0 - 1.0
            })
            .collect();
        a.push(&noise);

        let bands = a.plan().bands.clone();
        let pick = |hz: f64| bands.iter().position(|b| b.flo <= hz && hz < b.fhi).unwrap();
        let lo = pick(500.0);
        let hi = pick(4000.0);
        let rise = (a.bands_db()[hi] - a.bands_db()[lo]) as f64;
        // Three octaves is nine third-octaves, so about 9 dB.
        assert!(
            (rise - 9.0).abs() < 1.5,
            "white noise rose {rise:.2} dB over three octaves, expected about 9"
        );
    }

    #[test]
    fn peak_hold_only_rises() {
        let rate = 48000.0;
        let mut cfg = config(Fraction::Third, 8192);
        cfg.peak_hold = true;
        cfg.averaging = Averaging::Fast;
        let mut a = SpectrumAnalyser::new(cfg, rate);

        a.push(&sine(rate, 1000.0, 1.0, 8192 * 8));
        let held: Vec<f32> = a.peaks_db().to_vec();
        a.push(&vec![0.0f32; 8192 * 8]);
        for (before, now) in held.iter().zip(a.peaks_db()) {
            assert!(now >= before, "a held peak fell from {before} to {now}");
        }
        // ...while the live display follows the signal down.
        let idx = a
            .plan()
            .bands
            .iter()
            .position(|b| b.flo <= 1000.0 && 1000.0 < b.fhi)
            .unwrap();
        assert!(a.bands_db()[idx] < held[idx] - 20.0);
    }

    #[test]
    fn changing_averaging_keeps_the_band_table() {
        let rate = 48000.0;
        let mut a = SpectrumAnalyser::new(config(Fraction::Twelfth, 8192), rate);
        a.push(&sine(rate, 1000.0, 1.0, 8192 * 4));
        let before = a.plan().bands.len();

        let mut cfg = a.config();
        cfg.averaging = Averaging::Fast;
        a.reconfigure(cfg, rate);
        assert_eq!(a.plan().bands.len(), before);
        assert!(a.transforms() > 0, "a non-structural change restarted the analyser");
    }

    #[test]
    fn changing_resolution_rebuilds_the_band_table() {
        let rate = 48000.0;
        let mut a = SpectrumAnalyser::new(config(Fraction::Third, 8192), rate);
        let third = a.plan().bands.len();
        let mut cfg = a.config();
        cfg.fraction = Fraction::Twelfth;
        a.reconfigure(cfg, rate);
        assert!(a.plan().bands.len() > third * 3);
    }

    /// Interpolation has to beat the bin spacing, or it is not worth doing.
    /// 997 Hz at an 8192-point transform sits between bins that are 5.86 Hz
    /// apart, so anything better than about 1 Hz proves the parabola is working.
    #[test]
    fn dominant_frequency_is_interpolated_between_bins() {
        let rate = 48000.0;
        for tone in [100.0, 997.0, 1000.0, 4321.0] {
            let mut a = SpectrumAnalyser::new(config(Fraction::Third, 8192), rate);
            a.push(&sine(rate, tone, 0.5, 8192 * 4));
            let got = a.dominant_hz().expect("a tone should have a peak");
            assert!(
                (got - tone).abs() < 1.0,
                "{tone} Hz tone was located at {got:.2} Hz"
            );
        }
    }

    #[test]
    fn a_dc_offset_is_not_reported_as_the_dominant_tone() {
        let rate = 48000.0;
        let mut a = SpectrumAnalyser::new(config(Fraction::Third, 8192), rate);
        let tone = sine(rate, 1000.0, 0.2, 8192 * 4);
        let with_dc: Vec<f32> = tone.iter().map(|x| x + 0.9).collect();
        a.push(&with_dc);
        let got = a.dominant_hz().expect("still a peak");
        assert!(
            (got - 1000.0).abs() < 5.0,
            "DC offset dragged the dominant frequency to {got:.1} Hz"
        );
    }

    #[test]
    fn silence_has_no_dominant_frequency() {
        let rate = 48000.0;
        let mut a = SpectrumAnalyser::new(config(Fraction::Third, 4096), rate);
        a.push(&vec![0.0f32; 4096 * 2]);
        assert_eq!(a.dominant_hz(), None);
    }

    #[test]
    fn silence_reads_the_floor_and_not_nan() {
        let rate = 48000.0;
        let mut a = SpectrumAnalyser::new(config(Fraction::Third, 4096), rate);
        a.push(&vec![0.0f32; 4096 * 4]);
        assert!(a.bands_db().iter().all(|v| v.is_finite()));
        assert!(a.bands_db().iter().all(|&v| v < -100.0));
    }

    #[test]
    fn hop_controls_how_often_a_transform_runs() {
        let rate = 48000.0;
        let mut cfg = config(Fraction::Third, 4096);
        cfg.hop_fraction = 0.25;
        let mut a = SpectrumAnalyser::new(cfg, rate);
        a.push(&vec![0.0f32; 4096]);
        // 4096 samples at a 1024-sample hop is four transforms.
        assert_eq!(a.transforms(), 4);
        assert!((a.hop_seconds() - 1024.0 / rate).abs() < 1e-12);
    }

    #[test]
    fn an_invalid_config_falls_back_rather_than_panicking() {
        let rate = 48000.0;
        let cfg = SpectrumConfig {
            fft_size: 12345,
            hop_fraction: 0.0,
            ..config(Fraction::Third, 4096)
        };
        let a = SpectrumAnalyser::new(cfg, rate);
        assert_eq!(a.config().fft_size, 16384);
        assert!((a.config().hop_fraction - 0.5).abs() < 1e-12);
    }
}
