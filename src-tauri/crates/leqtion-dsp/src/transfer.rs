//! Dual-channel transfer function: magnitude, phase and coherence.
//!
//! Two signals go in — a **reference** (what was sent) and a **measurement**
//! (what came back) — and out comes the frequency response of whatever sits
//! between them.
//!
//! ```text
//!   reference ──▶ delay ──▶ ┐
//!                           ├──▶ per-slice FFT ──▶ Sxx, Syy, Sxy ──▶ H, γ²
//!   measurement ────────────┘
//! ```
//!
//! ## The estimator
//!
//! `H = Sxy / Sxx`, the H1 estimator, computed from **complex-averaged**
//! cross-spectra. Three details, and getting any of them wrong produces a
//! plausible-looking curve that is wrong:
//!
//! - The cross-spectrum is averaged as a complex number. Averaging magnitudes
//!   and phases separately throws away exactly the information that
//!   distinguishes a real response from noise, and makes coherence meaningless.
//! - Coherence `γ² = |Sxy|² / (Sxx·Syy)` is only defined *across averages*. A
//!   single frame always gives exactly 1, which is why nothing is reported
//!   until several frames are in — see [`MIN_FRAMES_FOR_COHERENCE`].
//! - The reference must be delay-compensated first. Phase without delay
//!   compensation is a steeply sloping line that says nothing about the system,
//!   only about how far away the microphone is.
//!
//! ## Multi-time-window
//!
//! One FFT length cannot serve 20 Hz and 16 kHz. At 48 kHz a 16384-point
//! transform gives 2.9 Hz bins: about right at 30 Hz, and absurdly narrow at
//! 10 kHz, where it buys nothing and costs stability. A short transform is the
//! reverse — fine at the top, useless in the bottom two octaves.
//!
//! So several transforms run in parallel, each serving a couple of octaves,
//! halving in length as frequency rises, and the results are stitched onto a
//! single set of points spaced at a fixed number per octave. Each slice's
//! length is chosen so its bin spacing is finer than the output point spacing
//! at the *bottom* of the range it serves — see [`build_slices`].

use realfft::num_complex::Complex;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::window::{Window, WindowKind};

/// Coherence is undefined on a single frame. Below this many, nothing is
/// reported rather than reporting a confident-looking 1.0.
pub const MIN_FRAMES_FOR_COHERENCE: u64 = 4;

/// Largest transform used. At 48 kHz this is 1.37 s and 0.73 Hz bins, which is
/// as far down as it is worth going: a longer window makes the measurement
/// slower to settle without telling anyone anything they can act on.
pub const MAX_FFT: usize = 65536;
/// Smallest transform. Below this the window is shorter than the impulse
/// response of most rooms and the result is dominated by the window, not the
/// system.
pub const MIN_FFT: usize = 256;

/// Each slice covers this many octaves before handing over to a shorter one.
const OCTAVES_PER_SLICE: f64 = 2.0;

/// Safety factor on the slice length.
///
/// A slice must resolve the output point spacing at the bottom of its range.
/// Point spacing at f is `f·(2^(1/ppo) − 1)`, so the transform needs at least
/// `fs / (f·(2^(1/ppo) − 1))` points. This multiplies that, so the bins are
/// comfortably finer than the points rather than exactly equal — interpolating
/// between two bins that are the same width as the output spacing smears the
/// answer.
const SLICE_MARGIN: f64 = 1.5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TfAveraging {
    /// Exponential, over roughly this many frames.
    Fast,
    Slow,
    Long,
    /// Linear running mean since the last reset — the one to use for a
    /// measurement that should stop moving.
    Infinite,
}

impl TfAveraging {
    pub const ALL: [TfAveraging; 4] = [
        TfAveraging::Fast,
        TfAveraging::Slow,
        TfAveraging::Long,
        TfAveraging::Infinite,
    ];

    pub fn label(self) -> &'static str {
        match self {
            TfAveraging::Fast => "Fast",
            TfAveraging::Slow => "Slow",
            TfAveraging::Long => "Long",
            TfAveraging::Infinite => "Infinite",
        }
    }

    /// Number of frames the exponential average is spread over.
    fn frames(self) -> Option<f64> {
        match self {
            TfAveraging::Fast => Some(4.0),
            TfAveraging::Slow => Some(16.0),
            TfAveraging::Long => Some(64.0),
            TfAveraging::Infinite => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferConfig {
    /// Output points per octave. 24 is the usual working figure.
    pub points_per_octave: u32,
    pub f_min: f64,
    pub f_max: f64,
    pub averaging: TfAveraging,
    pub window: WindowKind,
    /// Points below this coherence are reported but flagged, so a display can
    /// fade or hide them. 0 shows everything.
    pub coherence_floor: f32,
}

impl Default for TransferConfig {
    fn default() -> Self {
        TransferConfig {
            points_per_octave: 24,
            f_min: 20.0,
            f_max: 20000.0,
            averaging: TfAveraging::Slow,
            window: WindowKind::Hann,
            coherence_floor: 0.5,
        }
    }
}

impl TransferConfig {
    fn sanitised(mut self) -> Self {
        self.points_per_octave = self.points_per_octave.clamp(3, 96);
        self.f_min = self.f_min.clamp(1.0, 1000.0);
        self.f_max = self.f_max.clamp(self.f_min * 2.0, 96_000.0);
        self.coherence_floor = self.coherence_floor.clamp(0.0, 1.0);
        self
    }
}

/// A delay applied to the reference so it lines up with the measurement.
///
/// The measurement always arrives *later* than the reference — it has been
/// through a converter, some cable, some air and back — so it is the reference
/// that gets held back. Delaying the measurement instead would mean predicting
/// the future.
#[derive(Debug, Clone)]
struct DelayLine {
    buffer: Vec<f32>,
    write: usize,
    delay: usize,
}

impl DelayLine {
    fn new(max_samples: usize) -> Self {
        DelayLine {
            buffer: vec![0.0; max_samples.max(1)],
            write: 0,
            delay: 0,
        }
    }

    fn set(&mut self, delay: usize) {
        self.delay = delay.min(self.buffer.len() - 1);
    }

    fn delay(&self) -> usize {
        self.delay
    }

    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let n = self.buffer.len();
        self.buffer[self.write] = x;
        self.write = (self.write + 1) % n;
        let read = (self.write + n - self.delay - 1) % n;
        self.buffer[read]
    }

    fn clear(&mut self) {
        self.buffer.fill(0.0);
        self.write = 0;
    }
}

/// One transform length and the spectra accumulated at it.
struct Slice {
    fft_size: usize,
    /// Range of output frequencies this slice is responsible for.
    f_lo: f64,
    f_hi: f64,
    bin_hz: f64,

    fft: Arc<dyn RealToComplex<f64>>,
    window: Window,

    ring_ref: Vec<f64>,
    ring_meas: Vec<f64>,
    write: usize,
    since_hop: usize,
    hop: usize,

    scratch: Vec<f64>,
    spec_ref: Vec<Complex<f64>>,
    spec_meas: Vec<Complex<f64>>,

    /// Auto-spectrum of the reference, averaged.
    sxx: Vec<f64>,
    /// Auto-spectrum of the measurement, averaged.
    syy: Vec<f64>,
    /// Cross-spectrum, averaged as a complex number.
    sxy: Vec<Complex<f64>>,
    frames: u64,
}

impl Slice {
    fn new(fft_size: usize, f_lo: f64, f_hi: f64, sample_rate: f64, kind: WindowKind) -> Self {
        let mut planner = RealFftPlanner::<f64>::new();
        let fft = planner.plan_fft_forward(fft_size);
        let bins = fft.make_output_vec().len();

        Slice {
            fft_size,
            f_lo,
            f_hi,
            bin_hz: sample_rate / fft_size as f64,
            window: Window::new(kind, fft_size),
            ring_ref: vec![0.0; fft_size],
            ring_meas: vec![0.0; fft_size],
            write: 0,
            since_hop: 0,
            hop: fft_size / 2,
            scratch: vec![0.0; fft_size],
            spec_ref: vec![Complex::default(); bins],
            spec_meas: vec![Complex::default(); bins],
            sxx: vec![0.0; bins],
            syy: vec![0.0; bins],
            sxy: vec![Complex::default(); bins],
            frames: 0,
            fft,
        }
    }

    /// Feed one aligned sample pair. Returns true if a transform ran.
    #[inline]
    fn push(&mut self, reference: f32, measurement: f32, averaging: TfAveraging) -> bool {
        self.ring_ref[self.write] = reference as f64;
        self.ring_meas[self.write] = measurement as f64;
        self.write = (self.write + 1) % self.fft_size;
        self.since_hop += 1;
        if self.since_hop < self.hop {
            return false;
        }
        self.since_hop = 0;
        self.transform(averaging);
        true
    }

    fn transform(&mut self, averaging: TfAveraging) {
        let n = self.fft_size;

        for i in 0..n {
            let idx = (self.write + i) % n;
            self.scratch[i] = self.ring_ref[idx] * self.window.samples[i];
        }
        if self.fft.process(&mut self.scratch, &mut self.spec_ref).is_err() {
            return;
        }
        for i in 0..n {
            let idx = (self.write + i) % n;
            self.scratch[i] = self.ring_meas[idx] * self.window.samples[i];
        }
        if self
            .fft
            .process(&mut self.scratch, &mut self.spec_meas)
            .is_err()
        {
            return;
        }

        self.frames += 1;
        // Seed on the first frame rather than ramping up from zero, which would
        // otherwise read as a response that fades in.
        let alpha = match averaging.frames() {
            _ if self.frames == 1 => 1.0,
            Some(f) => 1.0 / f,
            None => 1.0 / self.frames as f64,
        };

        for k in 0..self.sxx.len() {
            let x = self.spec_ref[k];
            let y = self.spec_meas[k];
            // Sxy = conj(X)·Y. The conjugate goes on the *reference*, which is
            // what makes arg(H) positive when the measurement lags — i.e. what
            // makes a delay read as a delay rather than as a lead.
            let cross = x.conj() * y;
            let xx = x.norm_sqr();
            let yy = y.norm_sqr();

            self.sxx[k] += alpha * (xx - self.sxx[k]);
            self.syy[k] += alpha * (yy - self.syy[k]);
            let sxy = self.sxy[k];
            self.sxy[k] = sxy + (cross - sxy) * alpha;
        }
    }

    /// Linear interpolation of the accumulated spectra at a frequency.
    fn at(&self, hz: f64) -> (f64, f64, Complex<f64>) {
        let x = hz / self.bin_hz;
        let last = self.sxx.len() - 1;
        let k0 = (x.floor() as usize).min(last.saturating_sub(1));
        let t = (x - k0 as f64).clamp(0.0, 1.0);
        let k1 = (k0 + 1).min(last);
        (
            self.sxx[k0] * (1.0 - t) + self.sxx[k1] * t,
            self.syy[k0] * (1.0 - t) + self.syy[k1] * t,
            self.sxy[k0] * (1.0 - t) + self.sxy[k1] * t,
        )
    }

    fn reset(&mut self) {
        self.sxx.fill(0.0);
        self.syy.fill(0.0);
        self.sxy.fill(Complex::default());
        self.frames = 0;
        self.ring_ref.fill(0.0);
        self.ring_meas.fill(0.0);
        self.write = 0;
        self.since_hop = 0;
    }
}

/// Choose the transform lengths and what each is responsible for.
///
/// Built from the bottom up, because the lowest frequency sets the longest
/// window and everything else follows from it. Each slice serves
/// [`OCTAVES_PER_SLICE`] octaves and the next is a quarter of the length.
fn build_slices(
    sample_rate: f64,
    f_min: f64,
    f_max: f64,
    points_per_octave: u32,
    kind: WindowKind,
) -> Vec<Slice> {
    let point_ratio = 2f64.powf(1.0 / points_per_octave as f64) - 1.0;
    let mut slices = Vec::new();
    let mut f_lo = f_min;

    while f_lo < f_max && slices.len() < 12 {
        let f_hi = (f_lo * 2f64.powf(OCTAVES_PER_SLICE)).min(f_max);

        // Resolve the output point spacing at the bottom of this slice.
        let needed = SLICE_MARGIN * sample_rate / (f_lo * point_ratio);
        let size = next_power_of_two(needed as usize).clamp(MIN_FFT, MAX_FFT);

        slices.push(Slice::new(size, f_lo, f_hi, sample_rate, kind));
        if f_hi >= f_max {
            break;
        }
        f_lo = f_hi;
    }

    slices
}

fn next_power_of_two(n: usize) -> usize {
    let mut p = 1;
    while p < n {
        p <<= 1;
    }
    p
}

/// The output points, which only change when the configuration does.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferPlan {
    pub points_per_octave: u32,
    pub sample_rate: f64,
    /// Output frequencies, Hz.
    pub frequencies: Vec<f32>,
    /// Transform length serving each point — worth showing, because it explains
    /// why the bottom of the display settles more slowly than the top.
    pub fft_sizes: Vec<u32>,
    /// Longest window in use, seconds. How long a measurement takes to settle.
    pub longest_window_seconds: f64,
}

/// A transfer function reading.
///
/// Structure-of-arrays rather than a vector of structs: this crosses to the UI
/// thirty times a second and the arrays serialise to compact JSON, where a
/// vector of objects would repeat four field names per point.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferReading {
    pub magnitude_db: Vec<f32>,
    /// Wrapped to (−180, 180].
    pub phase_deg: Vec<f32>,
    /// 0 to 1. Below [`TransferConfig::coherence_floor`] the point should be
    /// shown faded or not at all.
    pub coherence: Vec<f32>,
    /// Frames averaged at the slice serving the middle of the range. A reading
    /// with few frames is not yet trustworthy whatever its coherence says.
    pub frames: u64,
    /// True once enough frames exist for coherence to mean anything.
    pub valid: bool,
    /// Delay currently applied to the reference.
    pub delay_samples: u32,
    pub delay_ms: f64,
}

/// What the delay finder concluded.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DelayEstimate {
    /// Total delay from reference to measurement, in samples, including what is
    /// already applied. Sub-sample interpolated.
    pub samples: f64,
    pub milliseconds: f64,
    /// Metres, at 343 m/s. The number anyone actually wants when aligning a
    /// microphone, and a good sanity check: if it says 40 m in a small room,
    /// the peak found was a reflection or noise.
    pub metres: f64,
    /// Height of the impulse peak relative to the rest of the response. Low
    /// means there was no clear arrival and the answer should not be trusted.
    pub confidence: f32,
}

pub struct TransferFunction {
    config: TransferConfig,
    sample_rate: f64,
    slices: Vec<Slice>,
    /// Full-band slice used only for the impulse response and delay finding.
    /// The stitched slices each cover two octaves, so none of them can produce
    /// a usable broadband impulse — a band-limited impulse has a peak several
    /// samples wide and locating it is guesswork.
    wide: Slice,
    inverse: Arc<dyn ComplexToReal<f64>>,
    delay: DelayLine,
    plan: TransferPlan,
    /// Which slice serves each output point.
    point_slice: Vec<usize>,
}

/// Speed of sound used to turn a delay into a distance, m/s at about 20 °C.
pub const SPEED_OF_SOUND: f64 = 343.0;

/// Longest delay that can be compensated. At 48 kHz this is 2.7 s, or about
/// 930 m of air — far beyond any real measurement, and enough that the delay
/// line is never the limit.
const MAX_DELAY_SAMPLES: usize = 131_072;

impl TransferFunction {
    pub fn new(config: TransferConfig, sample_rate: f64) -> Self {
        let config = config.sanitised();
        let slices = build_slices(
            sample_rate,
            config.f_min,
            config.f_max,
            config.points_per_octave,
            config.window,
        );

        let wide_size = MAX_FFT.min(next_power_of_two((sample_rate * 0.35) as usize));
        let wide = Slice::new(wide_size, config.f_min, config.f_max, sample_rate, config.window);
        let mut planner = RealFftPlanner::<f64>::new();
        let inverse = planner.plan_fft_inverse(wide_size);

        // Output points, geometrically spaced.
        let mut frequencies = Vec::new();
        let step = 2f64.powf(1.0 / config.points_per_octave as f64);
        let mut f = config.f_min;
        while f <= config.f_max * 1.0001 {
            frequencies.push(f as f32);
            f *= step;
        }

        let point_slice: Vec<usize> = frequencies
            .iter()
            .map(|&hz| {
                let hz = hz as f64;
                slices
                    .iter()
                    .position(|s| hz >= s.f_lo && hz < s.f_hi)
                    .unwrap_or(slices.len().saturating_sub(1))
            })
            .collect();

        let fft_sizes = point_slice
            .iter()
            .map(|&i| slices.get(i).map(|s| s.fft_size as u32).unwrap_or(0))
            .collect();

        let longest = slices.iter().map(|s| s.fft_size).max().unwrap_or(0);

        TransferFunction {
            plan: TransferPlan {
                points_per_octave: config.points_per_octave,
                sample_rate,
                frequencies,
                fft_sizes,
                longest_window_seconds: longest as f64 / sample_rate,
            },
            config,
            sample_rate,
            slices,
            wide,
            inverse,
            delay: DelayLine::new(MAX_DELAY_SAMPLES),
            point_slice,
        }
    }

    pub fn config(&self) -> TransferConfig {
        self.config
    }

    pub fn plan(&self) -> &TransferPlan {
        &self.plan
    }

    pub fn sample_rate(&self) -> f64 {
        self.sample_rate
    }

    pub fn delay_samples(&self) -> usize {
        self.delay.delay()
    }

    /// Set the delay applied to the reference, in samples.
    ///
    /// Changing it invalidates every average, because the spectra accumulated
    /// so far were measured against a differently aligned reference. Keeping
    /// them would blend two different measurements into one curve.
    pub fn set_delay_samples(&mut self, samples: usize) {
        if samples == self.delay.delay() {
            return;
        }
        self.delay.set(samples);
        self.delay.clear();
        self.reset();
    }

    pub fn reconfigure(&mut self, config: TransferConfig, sample_rate: f64) {
        let config = config.sanitised();
        let structural = config.points_per_octave != self.config.points_per_octave
            || config.f_min != self.config.f_min
            || config.f_max != self.config.f_max
            || config.window != self.config.window
            || (sample_rate - self.sample_rate).abs() > f64::EPSILON;

        if structural {
            let delay = self.delay.delay();
            *self = TransferFunction::new(config, sample_rate);
            self.delay.set(delay);
            return;
        }
        // Averaging and the coherence floor cost nothing to change live, and a
        // user adjusting them is not asking to restart the measurement.
        if config.averaging != self.config.averaging {
            self.reset();
        }
        self.config = config;
    }

    /// Feed one aligned sample pair.
    #[inline]
    pub fn push(&mut self, reference: f32, measurement: f32) {
        let reference = self.delay.process(reference);
        for slice in &mut self.slices {
            slice.push(reference, measurement, self.config.averaging);
        }
        self.wide.push(reference, measurement, self.config.averaging);
    }

    pub fn push_pairs(&mut self, reference: &[f32], measurement: &[f32]) {
        let n = reference.len().min(measurement.len());
        for i in 0..n {
            self.push(reference[i], measurement[i]);
        }
    }

    pub fn frames(&self) -> u64 {
        self.wide.frames
    }

    /// The current reading.
    pub fn reading(&self) -> TransferReading {
        let n = self.plan.frequencies.len();
        let mut magnitude_db = Vec::with_capacity(n);
        let mut phase_deg = Vec::with_capacity(n);
        let mut coherence = Vec::with_capacity(n);

        for (i, &hz) in self.plan.frequencies.iter().enumerate() {
            let Some(slice) = self.slices.get(self.point_slice[i]) else {
                magnitude_db.push(f32::NEG_INFINITY);
                phase_deg.push(0.0);
                coherence.push(0.0);
                continue;
            };
            let (sxx, syy, sxy) = slice.at(hz as f64);

            if sxx <= 1e-30 || slice.frames == 0 {
                // No reference energy here: the source is not driving this
                // frequency. Reporting a huge H because the denominator is tiny
                // would draw a wall of noise where there is simply no signal.
                magnitude_db.push(f32::NEG_INFINITY);
                phase_deg.push(0.0);
                coherence.push(0.0);
                continue;
            }

            let h = sxy / sxx;
            magnitude_db.push((20.0 * h.norm().max(1e-12).log10()) as f32);
            phase_deg.push(h.arg().to_degrees() as f32);

            let gamma = if syy <= 1e-30 || slice.frames < MIN_FRAMES_FOR_COHERENCE {
                0.0
            } else {
                (sxy.norm_sqr() / (sxx * syy)).clamp(0.0, 1.0)
            };
            coherence.push(gamma as f32);
        }

        TransferReading {
            magnitude_db,
            phase_deg,
            coherence,
            frames: self.wide.frames,
            valid: self.wide.frames >= MIN_FRAMES_FOR_COHERENCE,
            delay_samples: self.delay.delay() as u32,
            delay_ms: self.delay.delay() as f64 * 1000.0 / self.sample_rate,
            }
    }

    /// The impulse response, from the full-band slice.
    ///
    /// Computed as the inverse transform of `H = Sxy/Sxx` rather than of `Sxy`
    /// alone: dividing out the reference's own spectrum is what makes the
    /// result the response of the *system* rather than of the system convolved
    /// with whatever the source happened to be playing. Without it, pink noise
    /// — which has far more energy at the bottom — produces a smeared low-pass
    /// blob whose peak cannot be located to better than a millisecond.
    pub fn impulse_response(&self) -> Vec<f32> {
        let bins = self.wide.sxx.len();
        let mut spectrum = vec![Complex::<f64>::default(); bins];

        // Regularise the division. Bins where the reference has no energy would
        // otherwise divide by nearly zero and dominate the result entirely.
        let peak = self.wide.sxx.iter().cloned().fold(0.0f64, f64::max);
        let floor = peak * 1e-6;

        for (slot, (&sxx, &sxy)) in spectrum
            .iter_mut()
            .zip(self.wide.sxx.iter().zip(self.wide.sxy.iter()))
        {
            if sxx > floor {
                *slot = sxy / sxx;
            }
        }
        // DC and Nyquist must be real for the inverse transform to be valid.
        spectrum[0].im = 0.0;
        if let Some(last) = spectrum.last_mut() {
            last.im = 0.0;
        }

        let mut out = vec![0.0f64; self.wide.fft_size];
        if self.inverse.process(&mut spectrum, &mut out).is_err() {
            return Vec::new();
        }
        let scale = 1.0 / self.wide.fft_size as f64;
        out.iter().map(|&v| (v * scale) as f32).collect()
    }

    /// Locate the arrival.
    ///
    /// Returns the **total** delay from reference to measurement — what is
    /// already applied plus whatever residual is left — so the caller can pass
    /// it straight to [`Self::set_delay_samples`].
    pub fn find_delay(&self) -> Option<DelayEstimate> {
        if self.wide.frames < MIN_FRAMES_FOR_COHERENCE {
            return None;
        }
        let ir = self.impulse_response();
        if ir.is_empty() {
            return None;
        }

        // The impulse response is circular, so a negative delay appears at the
        // far end. Search the whole thing and treat the back half as negative.
        let n = ir.len();
        let mut best = 0usize;
        let mut best_v = 0.0f32;
        let mut sum = 0.0f64;
        for (i, &v) in ir.iter().enumerate() {
            let a = v.abs();
            sum += a as f64;
            if a > best_v {
                best_v = a;
                best = i;
            }
        }
        if best_v <= 0.0 {
            return None;
        }

        // Parabolic interpolation across the peak, for sub-sample resolution.
        let delta = if best > 0 && best + 1 < n {
            let l = ir[best - 1].abs() as f64;
            let c = ir[best].abs() as f64;
            let r = ir[best + 1].abs() as f64;
            let denom = l - 2.0 * c + r;
            if denom.abs() < 1e-18 {
                0.0
            } else {
                (0.5 * (l - r) / denom).clamp(-0.5, 0.5)
            }
        } else {
            0.0
        };

        let position = best as f64 + delta;
        let residual = if best > n / 2 {
            position - n as f64
        } else {
            position
        };

        // Peak height against the average, which is what distinguishes a clear
        // arrival from a response that is all noise.
        let mean = sum / n as f64;
        let confidence = if mean > 0.0 {
            ((best_v as f64 / mean) / 100.0).clamp(0.0, 1.0)
        } else {
            0.0
        };

        let samples = self.delay.delay() as f64 + residual;
        Some(DelayEstimate {
            samples,
            milliseconds: samples * 1000.0 / self.sample_rate,
            metres: samples / self.sample_rate * SPEED_OF_SOUND,
            confidence: confidence as f32,
        })
    }

    /// Discard every average and start again. Does not change the delay.
    pub fn reset(&mut self) {
        for s in &mut self.slices {
            s.reset();
        }
        self.wide.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::weighting::Biquad;

    const RATE: f64 = 48000.0;

    fn noise(n: usize, seed: u64) -> Vec<f32> {
        let mut state = seed | 1;
        (0..n)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                ((state >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0) as f32
            })
            .collect()
    }

    /// Feed the same signal to both channels through an optional transformation.
    fn run(
        tf: &mut TransferFunction,
        seconds: f64,
        mut transform: impl FnMut(f32) -> f32,
    ) {
        let n = (RATE * seconds) as usize;
        let src = noise(n, 0xC0FFEE);
        let meas: Vec<f32> = src.iter().map(|&x| transform(x)).collect();
        tf.push_pairs(&src, &meas);
    }

    fn config() -> TransferConfig {
        TransferConfig {
            points_per_octave: 24,
            f_min: 20.0,
            f_max: 20000.0,
            averaging: TfAveraging::Infinite,
            window: WindowKind::Hann,
            coherence_floor: 0.0,
        }
    }

    /// Read the reading at a frequency, picking the nearest output point.
    fn at(tf: &TransferFunction, r: &TransferReading, hz: f64) -> (f64, f64, f64) {
        let i = tf
            .plan()
            .frequencies
            .iter()
            .enumerate()
            .min_by(|a, b| {
                ((*a.1 as f64) - hz)
                    .abs()
                    .partial_cmp(&((*b.1 as f64) - hz).abs())
                    .unwrap()
            })
            .map(|(i, _)| i)
            .unwrap();
        (
            r.magnitude_db[i] as f64,
            r.phase_deg[i] as f64,
            r.coherence[i] as f64,
        )
    }

    /// The baseline every other test rests on: reference straight to
    /// measurement must be 0 dB, 0 degrees, coherence 1, everywhere.
    #[test]
    fn a_wire_is_flat_and_in_phase() {
        let mut tf = TransferFunction::new(config(), RATE);
        run(&mut tf, 6.0, |x| x);
        let r = tf.reading();
        assert!(r.valid);

        for hz in [30.0, 100.0, 1000.0, 5000.0, 15000.0] {
            let (mag, phase, coh) = at(&tf, &r, hz);
            assert!(mag.abs() < 0.2, "{hz} Hz: {mag:.3} dB, expected 0");
            assert!(phase.abs() < 2.0, "{hz} Hz: {phase:.2}°, expected 0");
            assert!(coh > 0.98, "{hz} Hz: coherence {coh:.4}");
        }
    }

    #[test]
    fn a_gain_reads_as_a_gain() {
        let mut tf = TransferFunction::new(config(), RATE);
        run(&mut tf, 6.0, |x| x * 0.5);
        let r = tf.reading();
        for hz in [50.0, 1000.0, 10000.0] {
            let (mag, phase, _) = at(&tf, &r, hz);
            assert!(
                (mag - -6.0206).abs() < 0.2,
                "{hz} Hz: {mag:.3} dB, expected -6.02"
            );
            assert!(phase.abs() < 2.0, "a gain must not rotate phase: {phase:.2}°");
        }
    }

    #[test]
    fn an_inversion_reads_as_180_degrees() {
        let mut tf = TransferFunction::new(config(), RATE);
        run(&mut tf, 6.0, |x| -x);
        let r = tf.reading();
        let (mag, phase, coh) = at(&tf, &r, 1000.0);
        assert!(mag.abs() < 0.2, "magnitude changed: {mag:.3} dB");
        assert!(
            phase.abs() > 178.0,
            "expected ±180°, got {phase:.2}°"
        );
        assert!(coh > 0.98);
    }

    /// A known filter, measured. This is the test that would catch a wrong
    /// estimator, a wrong window normalisation or a conjugate on the wrong side.
    #[test]
    fn a_lowpass_measures_as_that_lowpass() {
        let cutoff = 1000.0;
        let w = 2.0 * std::f64::consts::PI * cutoff;
        // First-order lowpass: 1/(s/w + 1).
        let mut filter = Biquad::bilinear_at([0.0, 0.0, 1.0], [0.0, 1.0 / w, 1.0], None, RATE);

        let mut tf = TransferFunction::new(config(), RATE);
        run(&mut tf, 8.0, |x| filter.process(x as f64) as f32);
        let r = tf.reading();

        // At the corner: -3 dB, -45°.
        let (mag, phase, coh) = at(&tf, &r, cutoff);
        assert!(
            (mag - -3.0).abs() < 0.6,
            "at the corner: {mag:.2} dB, expected -3"
        );
        assert!(
            (phase - -45.0).abs() < 6.0,
            "at the corner: {phase:.1}°, expected -45"
        );
        assert!(coh > 0.9, "coherence at the corner was {coh:.3}");

        // A decade up: -20 dB, approaching -90°.
        let (mag, phase, _) = at(&tf, &r, 10000.0);
        assert!(
            (mag - -20.0).abs() < 1.5,
            "a decade up: {mag:.2} dB, expected about -20"
        );
        assert!(
            (phase - -90.0).abs() < 10.0,
            "a decade up: {phase:.1}°, expected about -90"
        );

        // A decade down: flat and in phase.
        let (mag, phase, _) = at(&tf, &r, 100.0);
        assert!(mag.abs() < 0.6, "a decade down: {mag:.2} dB, expected 0");
        assert!((phase - -5.7).abs() < 6.0, "a decade down: {phase:.1}°");
    }

    /// Coherence is the honesty feature: it must collapse when the measurement
    /// is not caused by the reference.
    #[test]
    fn uncorrelated_noise_has_no_coherence() {
        let mut tf = TransferFunction::new(config(), RATE);
        let n = (RATE * 8.0) as usize;
        let a = noise(n, 0xAAAA);
        let b = noise(n, 0xBBBB);
        tf.push_pairs(&a, &b);

        let r = tf.reading();
        for hz in [100.0, 1000.0, 8000.0] {
            let (_, _, coh) = at(&tf, &r, hz);
            assert!(coh < 0.3, "{hz} Hz: coherence {coh:.3} on unrelated signals");
        }
    }

    #[test]
    fn adding_noise_to_the_measurement_lowers_coherence_without_moving_the_magnitude() {
        let n = (RATE * 8.0) as usize;
        let src = noise(n, 0x1234);
        let dirt = noise(n, 0x9999);

        let mut clean = TransferFunction::new(config(), RATE);
        clean.push_pairs(&src, &src);

        let contaminated: Vec<f32> = src
            .iter()
            .zip(&dirt)
            .map(|(&s, &d)| s + d * 0.5)
            .collect();
        let mut dirty = TransferFunction::new(config(), RATE);
        dirty.push_pairs(&src, &contaminated);

        let (mag_c, _, coh_c) = at(&clean, &clean.reading(), 1000.0);
        let (mag_d, _, coh_d) = at(&dirty, &dirty.reading(), 1000.0);

        assert!(coh_c > 0.98, "clean coherence {coh_c:.3}");
        assert!(
            (0.5..0.95).contains(&coh_d),
            "contaminated coherence {coh_d:.3} — expected clearly reduced but not zero"
        );
        // H1 is unbiased by noise on the *output*, so the magnitude should barely move.
        assert!(
            (mag_d - mag_c).abs() < 1.0,
            "magnitude moved {:.2} dB when only coherence should have changed",
            mag_d - mag_c
        );
    }

    #[test]
    fn coherence_is_not_reported_before_there_are_averages() {
        let mut tf = TransferFunction::new(config(), RATE);
        // Barely any signal — one frame at most on the long slices.
        run(&mut tf, 0.2, |x| x);
        let r = tf.reading();
        assert!(!r.valid, "a two-frame measurement should not claim validity");
        let (_, _, coh) = at(&tf, &r, 1000.0);
        assert_eq!(coh, 0.0, "coherence must not read 1.0 on a single frame");
    }

    /// A pure delay is what the delay finder exists to remove, so it is the
    /// thing to test it against.
    #[test]
    fn a_pure_delay_is_found() {
        for delay in [64usize, 480, 4800] {
            let mut tf = TransferFunction::new(config(), RATE);
            let n = (RATE * 6.0) as usize;
            let src = noise(n, 0xD00D);
            let mut meas = vec![0.0f32; n];
            meas[delay..].copy_from_slice(&src[..n - delay]);
            tf.push_pairs(&src, &meas);

            let est = tf.find_delay().expect("a clear arrival should be found");
            assert!(
                (est.samples - delay as f64).abs() < 1.0,
                "delay of {delay} samples measured as {:.2}",
                est.samples
            );
            assert!(
                (est.milliseconds - delay as f64 * 1000.0 / RATE).abs() < 0.05,
                "milliseconds disagreed with samples"
            );
            assert!(est.confidence > 0.1, "confidence {:.3}", est.confidence);
        }
    }

    /// And once compensated, the phase must come back flat — which is the whole
    /// point of finding it.
    #[test]
    fn compensating_a_delay_flattens_the_phase() {
        let delay = 512usize;
        let n = (RATE * 6.0) as usize;
        let src = noise(n, 0xF00D);
        let mut meas = vec![0.0f32; n];
        meas[delay..].copy_from_slice(&src[..n - delay]);

        let mut tf = TransferFunction::new(config(), RATE);
        tf.push_pairs(&src, &meas);

        // Before: phase rotates steeply with frequency.
        let before = tf.reading();
        let (_, p1, _) = at(&tf, &before, 2000.0);
        let (_, p2, _) = at(&tf, &before, 2200.0);
        assert!(
            (p1 - p2).abs() > 20.0,
            "an uncompensated delay should rotate phase quickly"
        );

        let est = tf.find_delay().unwrap();
        tf.set_delay_samples(est.samples.round() as usize);
        tf.push_pairs(&src, &meas);

        let after = tf.reading();
        for hz in [200.0, 1000.0, 5000.0, 12000.0] {
            let (mag, phase, coh) = at(&tf, &after, hz);
            assert!(
                phase.abs() < 8.0,
                "{hz} Hz: phase {phase:.2}° after compensation"
            );
            assert!(mag.abs() < 0.4, "{hz} Hz: {mag:.2} dB");
            assert!(coh > 0.9, "{hz} Hz: coherence {coh:.3}");
        }
    }

    #[test]
    fn changing_the_delay_discards_the_averages() {
        let mut tf = TransferFunction::new(config(), RATE);
        run(&mut tf, 4.0, |x| x);
        assert!(tf.frames() > 0);
        tf.set_delay_samples(1000);
        assert_eq!(tf.frames(), 0, "averages survived a delay change");
        assert_eq!(tf.delay_samples(), 1000);
    }

    #[test]
    fn silence_on_the_reference_reports_nothing_rather_than_noise() {
        let mut tf = TransferFunction::new(config(), RATE);
        let n = (RATE * 4.0) as usize;
        let quiet = vec![0.0f32; n];
        let meas = noise(n, 0x5151);
        tf.push_pairs(&quiet, &meas);

        let r = tf.reading();
        assert!(r.magnitude_db.iter().all(|v| v.is_infinite() || *v < -100.0));
        assert!(r.coherence.iter().all(|&c| c == 0.0));
        assert!(r.phase_deg.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn phase_is_wrapped_into_a_sensible_range() {
        let mut tf = TransferFunction::new(config(), RATE);
        let delay = 300usize;
        let n = (RATE * 4.0) as usize;
        let src = noise(n, 0x2222);
        let mut meas = vec![0.0f32; n];
        meas[delay..].copy_from_slice(&src[..n - delay]);
        tf.push_pairs(&src, &meas);

        let r = tf.reading();
        assert!(
            r.phase_deg.iter().all(|&p| (-180.5..=180.5).contains(&p)),
            "phase escaped ±180°"
        );
    }

    /// The slices must actually differ, and each must resolve the points it
    /// serves — otherwise the whole multi-time-window apparatus is decoration.
    #[test]
    fn slices_are_long_at_the_bottom_and_short_at_the_top() {
        let tf = TransferFunction::new(config(), RATE);
        let sizes: Vec<u32> = tf.plan().fft_sizes.clone();
        assert!(sizes.len() > 100);
        assert!(
            sizes[0] > *sizes.last().unwrap(),
            "the bottom of the range should use a longer transform than the top: {:?} vs {:?}",
            sizes[0],
            sizes.last()
        );

        // Every point's transform must resolve the spacing to the next point.
        let freqs = &tf.plan().frequencies;
        for i in 0..freqs.len() - 1 {
            let spacing = (freqs[i + 1] - freqs[i]) as f64;
            let bin_hz = RATE / sizes[i] as f64;
            assert!(
                bin_hz <= spacing * 1.35,
                "at {:.0} Hz the transform gives {bin_hz:.2} Hz bins for {spacing:.2} Hz spacing",
                freqs[i]
            );
        }
    }

    #[test]
    fn the_plan_describes_how_long_it_takes_to_settle() {
        let tf = TransferFunction::new(config(), RATE);
        let seconds = tf.plan().longest_window_seconds;
        assert!(
            (0.2..3.0).contains(&seconds),
            "longest window is {seconds:.3} s"
        );
    }

    #[test]
    fn reconfiguring_averaging_does_not_rebuild_the_plan() {
        let mut tf = TransferFunction::new(config(), RATE);
        let before = tf.plan().frequencies.len();
        let mut c = tf.config();
        c.coherence_floor = 0.8;
        tf.reconfigure(c, RATE);
        assert_eq!(tf.plan().frequencies.len(), before);
    }

    #[test]
    fn reconfiguring_resolution_rebuilds_the_plan_and_keeps_the_delay() {
        let mut tf = TransferFunction::new(config(), RATE);
        tf.set_delay_samples(777);
        let mut c = tf.config();
        c.points_per_octave = 48;
        tf.reconfigure(c, RATE);
        assert!(tf.plan().frequencies.len() > 200);
        assert_eq!(tf.delay_samples(), 777, "a resolution change lost the delay");
    }

    #[test]
    fn nonsense_configuration_is_clamped_rather_than_panicking() {
        let c = TransferConfig {
            points_per_octave: 100_000,
            f_min: -5.0,
            f_max: 1.0,
            coherence_floor: 9.0,
            ..config()
        };
        let tf = TransferFunction::new(c, RATE);
        assert!(!tf.plan().frequencies.is_empty());
        assert!(tf.config().points_per_octave <= 96);
        assert!(tf.config().f_max > tf.config().f_min);
    }
}
