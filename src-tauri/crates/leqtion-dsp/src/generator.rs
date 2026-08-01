//! Signal generation: the source half of a measurement.
//!
//! Everything here is sample-accurate and deterministic given a seed, which is
//! what lets the tests assert on the *spectrum* of the output rather than on
//! whether it merely produced numbers.
//!
//! ## Clicks
//!
//! Every parameter that a person can move while the generator is running —
//! level, frequency — is ramped rather than assigned. A level jump is a step
//! discontinuity, which is a click, which through a PA at measurement level is
//! a genuinely unpleasant thing to do to a room full of people and to a driver.
//! [`RAMP_SECONDS`] is the time constant; it is short enough to feel immediate
//! and long enough that nothing snaps.
//!
//! ## Levels
//!
//! Levels are **dBFS RMS**, on the same convention as the rest of the crate: a
//! full-scale sine is 0 dBFS. That means pink noise at 0 dBFS would clip
//! constantly — noise has a crest factor of about 12 dB — so the generator
//! defaults to −20 dBFS and reports the peak headroom it expects to need.

use serde::{Deserialize, Serialize};

use crate::bands::FULL_SCALE_SINE_OFFSET_DB;

/// Time constant for level and frequency ramps.
pub const RAMP_SECONDS: f64 = 0.02;

/// Default output level, dBFS RMS.
///
/// −20 dBFS leaves about 8 dB of headroom above pink noise's crest factor, so
/// the generator does not clip on its own peaks, and it is a sane level to send
/// into an unknown system by accident.
pub const DEFAULT_LEVEL_DBFS: f64 = -20.0;

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Signal {
    /// Silence. Still runs the stream, so the output stays open and the
    /// interface does not click on every mute.
    #[default]
    Off,
    Sine {
        hz: f64,
    },
    /// Equal energy per hertz.
    White,
    /// Equal energy per octave — flat on any constant-percentage-bandwidth
    /// display, which is what makes it the right noise for an RTA.
    Pink,
    /// Logarithmic sweep, repeating. `seconds` is one pass.
    Sweep {
        from_hz: f64,
        to_hz: f64,
        seconds: f64,
    },
}

impl Signal {
    pub fn label(&self) -> &'static str {
        match self {
            Signal::Off => "Off",
            Signal::Sine { .. } => "Sine",
            Signal::White => "White noise",
            Signal::Pink => "Pink noise",
            Signal::Sweep { .. } => "Sweep",
        }
    }

    /// Typical crest factor, dB — peak above RMS.
    ///
    /// Used to warn about clipping before it happens rather than after. A sine
    /// is 3 dB; Gaussian-ish noise is unbounded in principle but 12 dB covers
    /// it in practice and is the figure noise generators are specified at.
    pub fn crest_factor_db(&self) -> f64 {
        match self {
            Signal::Off => 0.0,
            Signal::Sine { .. } | Signal::Sweep { .. } => FULL_SCALE_SINE_OFFSET_DB,
            Signal::White | Signal::Pink => 12.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeneratorConfig {
    pub signal: Signal,
    /// Output level, dBFS RMS.
    pub level_dbfs: f64,
    /// High-pass the noise at this frequency. `None` leaves it alone.
    #[serde(default)]
    pub high_pass_hz: Option<f64>,
    /// Low-pass the noise at this frequency.
    #[serde(default)]
    pub low_pass_hz: Option<f64>,
}

impl Default for GeneratorConfig {
    fn default() -> Self {
        GeneratorConfig {
            signal: Signal::Off,
            level_dbfs: DEFAULT_LEVEL_DBFS,
            high_pass_hz: None,
            low_pass_hz: None,
        }
    }
}

impl GeneratorConfig {
    /// Peak level this configuration is expected to reach, dBFS.
    ///
    /// Above 0 means it will clip. The UI shows this next to the level control,
    /// because "pink noise at −6 dBFS" sounds conservative and is not.
    pub fn expected_peak_dbfs(&self) -> f64 {
        self.level_dbfs + self.signal.crest_factor_db() - FULL_SCALE_SINE_OFFSET_DB
    }
}

/// A one-pole smoother, used for every parameter a person can move live.
#[derive(Debug, Clone, Copy)]
struct Ramp {
    current: f64,
    target: f64,
    alpha: f64,
}

impl Ramp {
    fn new(value: f64, sample_rate: f64) -> Self {
        Ramp {
            current: value,
            target: value,
            alpha: 1.0 - (-1.0 / (sample_rate * RAMP_SECONDS)).exp(),
        }
    }

    fn set(&mut self, target: f64) {
        self.target = target;
    }

    /// Jump without ramping. Only for a change that is inaudible anyway,
    /// such as starting from silence.
    fn snap(&mut self, value: f64) {
        self.current = value;
        self.target = value;
    }

    #[inline]
    fn next(&mut self) -> f64 {
        self.current += self.alpha * (self.target - self.current);
        self.current
    }
}

/// Brings the pinking filter back to unit variance for unit-variance input.
///
/// The filter has an RMS gain of 3.0481, measured over four million samples;
/// this is its reciprocal. It is a *measured* constant rather than a derived
/// one because the coefficients below are themselves a published numerical fit
/// with no closed form. It does not depend on the sample rate: the filter is
/// defined in z, so its noise gain is a fixed integral over the normalised
/// frequency axis whatever the rate happens to be.
///
/// `noise_comes_out_at_the_level_asked_for` is what holds this honest — get it
/// wrong and pink noise is simply the wrong level, which is invisible until
/// someone compares against another generator.
const PINK_NORMALISATION: f64 = 0.328_075;

/// Paul Kellett's refined pinking filter.
///
/// White noise through this is within about ±0.05 dB of a true −3 dB/octave
/// slope from 10 Hz to 20 kHz, which is far better than the well-known
/// three-pole version and cheap enough not to care. The coefficients are not
/// derivable from anything — they are a published fit — so they are written out
/// rather than computed, and `pink_noise_is_flat_per_octave` is what proves the
/// implementation right.
#[derive(Debug, Clone, Copy, Default)]
struct Pinking {
    b0: f64,
    b1: f64,
    b2: f64,
    b3: f64,
    b4: f64,
    b5: f64,
    b6: f64,
}

impl Pinking {
    #[inline]
    fn process(&mut self, white: f64) -> f64 {
        self.b0 = 0.99886 * self.b0 + white * 0.0555179;
        self.b1 = 0.99332 * self.b1 + white * 0.0750759;
        self.b2 = 0.96900 * self.b2 + white * 0.1538520;
        self.b3 = 0.86650 * self.b3 + white * 0.3104856;
        self.b4 = 0.55000 * self.b4 + white * 0.5329522;
        self.b5 = -0.7616 * self.b5 - white * 0.0168980;
        let out = self.b0 + self.b1 + self.b2 + self.b3 + self.b4 + self.b5 + self.b6 + white * 0.5362;
        self.b6 = white * 0.115926;
        out * PINK_NORMALISATION
    }
}

/// xorshift128+. Deterministic, fast, and good enough for noise — this is a
/// signal generator, not a cryptosystem, and a reproducible sequence is
/// actively useful because it makes the spectrum tests repeatable.
#[derive(Debug, Clone, Copy)]
struct Rng {
    s0: u64,
    s1: u64,
}

impl Rng {
    fn new(seed: u64) -> Self {
        // splitmix64 to spread a small seed across both words; seeding a
        // xorshift with a low-entropy value gives a poor first few thousand
        // samples, which would land right in the middle of a short test.
        let mut z = seed.wrapping_add(0x9E3779B97F4A7C15);
        let mut next = || {
            z = z.wrapping_add(0x9E3779B97F4A7C15);
            let mut x = z;
            x = (x ^ (x >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
            x = (x ^ (x >> 27)).wrapping_mul(0x94D049BB133111EB);
            x ^ (x >> 31)
        };
        Rng {
            s0: next(),
            s1: next(),
        }
    }

    #[inline]
    fn next_u64(&mut self) -> u64 {
        let mut x = self.s0;
        let y = self.s1;
        self.s0 = y;
        x ^= x << 23;
        self.s1 = x ^ y ^ (x >> 17) ^ (y >> 26);
        self.s1.wrapping_add(y)
    }

    /// Uniform in [-1, 1).
    ///
    /// `>> 11` leaves 53 bits, which is exactly the mantissa of an f64, so the
    /// divisor must be 2^53 to land in [0, 1). Dividing by 2^52 instead gives
    /// [-1, 3): a signal with a mean of 1 and 11 dB too much energy, which
    /// reads as a noise generator that is simply too loud until you notice the
    /// enormous DC component the pinking filter is amplifying.
    #[inline]
    fn next_uniform(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
    }

    /// Approximately Gaussian, unit variance.
    ///
    /// The sum of three uniforms rather than a Box-Muller pair: it needs no
    /// transcendentals, and the difference matters not at all for a noise
    /// source whose spectrum is what is being measured. Variance of one
    /// uniform on [-1,1) is 1/3, so three of them sum to variance 1.
    #[inline]
    fn next_gaussian(&mut self) -> f64 {
        self.next_uniform() + self.next_uniform() + self.next_uniform()
    }
}

/// A simple one-pole high-pass and low-pass pair for band-limiting noise.
#[derive(Debug, Clone, Copy, Default)]
struct BandLimit {
    hp_state: f64,
    hp_alpha: f64,
    lp_state: f64,
    lp_alpha: f64,
    hp_on: bool,
    lp_on: bool,
}

impl BandLimit {
    fn configure(&mut self, hp: Option<f64>, lp: Option<f64>, sample_rate: f64) {
        let nyquist = sample_rate * 0.5;
        self.hp_on = matches!(hp, Some(f) if f > 0.0 && f < nyquist);
        self.lp_on = matches!(lp, Some(f) if f > 0.0 && f < nyquist);
        if let Some(f) = hp {
            self.hp_alpha = 1.0 - (-2.0 * std::f64::consts::PI * f / sample_rate).exp();
        }
        if let Some(f) = lp {
            self.lp_alpha = 1.0 - (-2.0 * std::f64::consts::PI * f / sample_rate).exp();
        }
    }

    #[inline]
    fn process(&mut self, x: f64) -> f64 {
        let mut v = x;
        if self.hp_on {
            self.hp_state += self.hp_alpha * (v - self.hp_state);
            v -= self.hp_state;
        }
        if self.lp_on {
            self.lp_state += self.lp_alpha * (v - self.lp_state);
            v = self.lp_state;
        }
        v
    }
}

pub struct Generator {
    config: GeneratorConfig,
    sample_rate: f64,
    level: Ramp,
    frequency: Ramp,
    /// Sine phase, radians, kept in [0, 2π) so it never loses precision on a
    /// generator left running for hours.
    phase: f64,
    sweep_position: f64,
    rng: Rng,
    pinking: Pinking,
    band: BandLimit,
}

impl Generator {
    pub fn new(config: GeneratorConfig, sample_rate: f64) -> Self {
        let mut g = Generator {
            sample_rate,
            level: Ramp::new(0.0, sample_rate),
            frequency: Ramp::new(1000.0, sample_rate),
            phase: 0.0,
            sweep_position: 0.0,
            rng: Rng::new(0x5EED),
            pinking: Pinking::default(),
            band: BandLimit::default(),
            config,
        };
        // Start from silence and ramp up, whatever the level says. Opening an
        // output already at −6 dBFS is how a driver gets damaged.
        g.level.snap(0.0);
        g.apply(config);
        g
    }

    pub fn config(&self) -> GeneratorConfig {
        self.config
    }

    pub fn sample_rate(&self) -> f64 {
        self.sample_rate
    }

    /// Change the settings. Level and frequency ramp; the signal type does not,
    /// so switching from sine to pink is a discontinuity — but muting first is
    /// the user's choice to make, and cross-fading signal types would hide a
    /// change they asked for.
    pub fn apply(&mut self, config: GeneratorConfig) {
        self.config = config;
        let amplitude = match config.signal {
            Signal::Off => 0.0,
            _ => db_to_amplitude(config.level_dbfs),
        };
        self.level.set(amplitude);
        if let Signal::Sine { hz } = config.signal {
            self.frequency.set(hz.clamp(1.0, self.sample_rate * 0.49));
        }
        self.band
            .configure(config.high_pass_hz, config.low_pass_hz, self.sample_rate);
    }

    /// Fill a mono buffer.
    pub fn fill(&mut self, out: &mut [f32]) {
        for sample in out.iter_mut() {
            *sample = self.next() as f32;
        }
    }

    /// Fill an interleaved output buffer, writing only to `channel`.
    ///
    /// Every other channel is zeroed. That is deliberate: leaving them
    /// untouched would replay whatever the driver left in the buffer, which on
    /// some hosts is the previous block and sounds like a stutter.
    pub fn fill_interleaved(&mut self, out: &mut [f32], channels: usize, channel: usize) {
        if channels == 0 {
            return;
        }
        let target = channel.min(channels - 1);
        let frames = out.len() / channels;
        for f in 0..frames {
            let v = self.next() as f32;
            for c in 0..channels {
                out[f * channels + c] = if c == target { v } else { 0.0 };
            }
        }
    }

    #[inline]
    fn next(&mut self) -> f64 {
        let level = self.level.next();
        if level <= 0.0 && matches!(self.config.signal, Signal::Off) {
            return 0.0;
        }

        let raw = match self.config.signal {
            Signal::Off => 0.0,

            Signal::Sine { .. } => {
                let hz = self.frequency.next();
                self.phase += 2.0 * std::f64::consts::PI * hz / self.sample_rate;
                if self.phase >= 2.0 * std::f64::consts::PI {
                    self.phase -= 2.0 * std::f64::consts::PI;
                }
                // A sine's RMS is 1/√2 of its amplitude, and `level` is an RMS
                // target, so scale up to keep the convention consistent with
                // the noise sources.
                self.phase.sin() * std::f64::consts::SQRT_2
            }

            Signal::White => self.band.process(self.rng.next_gaussian()),

            Signal::Pink => {
                let white = self.rng.next_gaussian();
                self.band.process(self.pinking.process(white))
            }

            Signal::Sweep {
                from_hz,
                to_hz,
                seconds,
            } => {
                let seconds = seconds.max(0.1);
                let from = from_hz.clamp(1.0, self.sample_rate * 0.49);
                let to = to_hz.clamp(1.0, self.sample_rate * 0.49);
                let t = self.sweep_position;

                // Exponential sweep: frequency rises geometrically, so the
                // sweep spends equal time per octave. Phase is the integral of
                // that, which is why it is accumulated rather than computed
                // from t directly — computing it would drift out of phase
                // continuity every time the sweep wrapped.
                let hz = from * (to / from).powf(t);
                self.phase += 2.0 * std::f64::consts::PI * hz / self.sample_rate;
                if self.phase >= 2.0 * std::f64::consts::PI {
                    self.phase -= 2.0 * std::f64::consts::PI;
                }
                self.sweep_position += 1.0 / (seconds * self.sample_rate);
                if self.sweep_position >= 1.0 {
                    self.sweep_position -= 1.0;
                }
                self.phase.sin() * std::f64::consts::SQRT_2
            }
        };

        // Hard-limit at full scale. The generator should never be the thing
        // that produces an out-of-range sample, whatever the level is set to —
        // a converter fed >1.0 does something unpredictable and vendor-specific.
        (raw * level).clamp(-1.0, 1.0)
    }

    /// Reset the noise sequence and sweep. Makes a measurement repeatable.
    pub fn reseed(&mut self, seed: u64) {
        self.rng = Rng::new(seed);
        self.pinking = Pinking::default();
        self.sweep_position = 0.0;
        self.phase = 0.0;
    }
}

/// dBFS RMS → linear amplitude, on the full-scale-sine convention.
pub fn db_to_amplitude(dbfs: f64) -> f64 {
    if dbfs <= -200.0 {
        return 0.0;
    }
    10f64.powf((dbfs - FULL_SCALE_SINE_OFFSET_DB) / 20.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bands::Fraction;
    use crate::spectrum::{Averaging, SpectrumAnalyser, SpectrumConfig};
    use crate::spl::{mean_square_to_dbfs, LevelDetector, TimeWeighting};
    use crate::window::WindowKind;

    const RATE: f64 = 48000.0;

    fn generate(config: GeneratorConfig, seconds: f64) -> Vec<f32> {
        let mut g = Generator::new(config, RATE);
        let mut buf = vec![0.0f32; (RATE * seconds) as usize];
        g.fill(&mut buf);
        buf
    }

    fn measure_dbfs(samples: &[f32]) -> f64 {
        // Skip the first 100 ms so the level ramp is not counted.
        let skip = (RATE * 0.1) as usize;
        let mut d = LevelDetector::new(TimeWeighting::Slow, RATE);
        d.push(&samples[skip..]);
        let s: f64 = samples[skip..].iter().map(|&x| (x as f64) * (x as f64)).sum();
        mean_square_to_dbfs(s / (samples.len() - skip) as f64)
    }

    fn analyse(samples: &[f32], fraction: Fraction, fft_size: usize) -> SpectrumAnalyser {
        let mut a = SpectrumAnalyser::new(
            SpectrumConfig {
                fraction,
                fft_size,
                window: WindowKind::Hann,
                hop_fraction: 0.5,
                averaging: Averaging::Infinite,
                peak_hold: false,
            },
            RATE,
        );
        a.push(samples);
        a
    }

    #[test]
    fn a_sine_comes_out_at_the_level_asked_for() {
        for level in [-6.0, -20.0, -40.0] {
            let out = generate(
                GeneratorConfig {
                    signal: Signal::Sine { hz: 1000.0 },
                    level_dbfs: level,
                    ..Default::default()
                },
                2.0,
            );
            let got = measure_dbfs(&out);
            assert!(
                (got - level).abs() < 0.1,
                "asked for {level} dBFS, measured {got:.2}"
            );
        }
    }

    #[test]
    fn noise_comes_out_at_the_level_asked_for() {
        for signal in [Signal::White, Signal::Pink] {
            let out = generate(
                GeneratorConfig {
                    signal,
                    level_dbfs: -20.0,
                    ..Default::default()
                },
                4.0,
            );
            let got = measure_dbfs(&out);
            assert!(
                (got - -20.0).abs() < 0.6,
                "{:?}: asked for -20 dBFS, measured {got:.2}",
                signal
            );
        }
    }

    /// The defining property of pink noise, and the reason it is the noise an
    /// RTA is verified with: equal energy per octave means a
    /// constant-percentage-bandwidth display draws a flat line. A tilt here is
    /// either the pinking filter or the band integrator, and the band
    /// integrator has its own tests, so a failure points at this file.
    #[test]
    fn pink_noise_is_flat_per_octave() {
        let out = generate(
            GeneratorConfig {
                signal: Signal::Pink,
                level_dbfs: -20.0,
                ..Default::default()
            },
            20.0,
        );
        let a = analyse(&out, Fraction::Third, 8192);

        // Compare only where the transform resolves the bands properly.
        let bands: Vec<(f64, f32)> = a
            .plan()
            .bands
            .iter()
            .zip(a.bands_db())
            .filter(|(b, _)| b.fc >= 50.0 && b.fc <= 12000.0)
            .map(|(b, &db)| (b.fc, db))
            .collect();
        assert!(bands.len() > 20);

        let mean: f64 = bands.iter().map(|(_, db)| *db as f64).sum::<f64>() / bands.len() as f64;
        for (fc, db) in &bands {
            assert!(
                ((*db as f64) - mean).abs() < 1.5,
                "pink noise is {:.2} dB off flat at {fc:.0} Hz (mean {mean:.2})",
                (*db as f64) - mean
            );
        }

        // And check the slope explicitly: eight octaves of drift would still
        // pass a per-band tolerance if it crept, so fit the two ends.
        let low: f64 = bands.iter().take(6).map(|(_, d)| *d as f64).sum::<f64>() / 6.0;
        let high: f64 = bands.iter().rev().take(6).map(|(_, d)| *d as f64).sum::<f64>() / 6.0;
        assert!(
            (high - low).abs() < 1.0,
            "pink noise drifts {:.2} dB from bottom to top",
            high - low
        );
    }

    /// White noise has equal energy per hertz, so on the same display it must
    /// rise at 1 dB per third-octave. This is the control for the test above:
    /// if both were flat, the pinking filter would not be doing anything.
    #[test]
    fn white_noise_rises_three_db_per_octave() {
        let out = generate(
            GeneratorConfig {
                signal: Signal::White,
                level_dbfs: -20.0,
                ..Default::default()
            },
            20.0,
        );
        let a = analyse(&out, Fraction::Third, 8192);
        let pick = |hz: f64| {
            a.plan()
                .bands
                .iter()
                .position(|b| b.flo <= hz && hz < b.fhi)
                .map(|i| a.bands_db()[i] as f64)
                .unwrap()
        };
        let rise = pick(8000.0) - pick(500.0);
        // Four octaves at 3 dB each.
        assert!(
            (rise - 12.0).abs() < 1.5,
            "white noise rose {rise:.2} dB over four octaves, expected about 12"
        );
    }

    #[test]
    fn a_sine_lands_on_its_own_frequency() {
        for hz in [50.0, 1000.0, 6300.0] {
            let out = generate(
                GeneratorConfig {
                    signal: Signal::Sine { hz },
                    level_dbfs: -12.0,
                    ..Default::default()
                },
                2.0,
            );
            let a = analyse(&out, Fraction::Third, 16384);
            let got = a.dominant_hz().expect("a sine should have a peak");
            assert!(
                (got - hz).abs() / hz < 0.01,
                "asked for {hz} Hz, measured {got:.1}"
            );
        }
    }

    #[test]
    fn the_output_never_leaves_full_scale() {
        // Deliberately absurd: pink noise at 0 dBFS RMS will clip constantly,
        // and the generator must clamp rather than hand the converter
        // out-of-range samples.
        let out = generate(
            GeneratorConfig {
                signal: Signal::Pink,
                level_dbfs: 0.0,
                ..Default::default()
            },
            2.0,
        );
        assert!(out.iter().all(|&x| (-1.0..=1.0).contains(&x)));
        assert!(out.iter().all(|x| x.is_finite()));
    }

    #[test]
    fn the_expected_peak_warns_before_it_clips() {
        let sine = GeneratorConfig {
            signal: Signal::Sine { hz: 1000.0 },
            level_dbfs: -1.0,
            ..Default::default()
        };
        assert!(sine.expected_peak_dbfs() < 0.0, "a sine at -1 dBFS is fine");

        let pink = GeneratorConfig {
            signal: Signal::Pink,
            level_dbfs: -6.0,
            ..Default::default()
        };
        assert!(
            pink.expected_peak_dbfs() > 0.0,
            "pink noise at -6 dBFS RMS should be flagged as clipping, got {:.1}",
            pink.expected_peak_dbfs()
        );
    }

    /// Starting the generator must not produce a step. A click into a PA at
    /// measurement level is both unpleasant and capable of damaging a driver.
    #[test]
    fn output_starts_from_silence_without_a_step() {
        let mut g = Generator::new(
            GeneratorConfig {
                signal: Signal::Pink,
                level_dbfs: -6.0,
                ..Default::default()
            },
            RATE,
        );
        let mut buf = vec![0.0f32; 256];
        g.fill(&mut buf);
        assert!(buf[0].abs() < 0.01, "first sample was {}", buf[0]);

        // And the step between consecutive samples stays small through the ramp.
        let mut whole = vec![0.0f32; (RATE * 0.5) as usize];
        let mut g2 = Generator::new(
            GeneratorConfig {
                signal: Signal::Sine { hz: 50.0 },
                level_dbfs: -6.0,
                ..Default::default()
            },
            RATE,
        );
        g2.fill(&mut whole);
        let worst = whole
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0f32, f32::max);
        // A 50 Hz sine at -6 dBFS moves at most 0.066 per sample of its own
        // accord; anything much beyond that is a discontinuity.
        assert!(worst < 0.1, "worst sample-to-sample step was {worst}");
    }

    #[test]
    fn changing_level_ramps_rather_than_jumps() {
        let mut g = Generator::new(
            GeneratorConfig {
                signal: Signal::Sine { hz: 1000.0 },
                level_dbfs: -40.0,
                ..Default::default()
            },
            RATE,
        );
        let mut settle = vec![0.0f32; (RATE * 0.5) as usize];
        g.fill(&mut settle);

        g.apply(GeneratorConfig {
            signal: Signal::Sine { hz: 1000.0 },
            level_dbfs: -6.0,
            ..Default::default()
        });
        let mut after = vec![0.0f32; 512];
        g.fill(&mut after);

        let worst = after
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0f32, f32::max);
        assert!(worst < 0.15, "a 34 dB level change stepped by {worst}");
    }

    #[test]
    fn a_sweep_covers_its_range() {
        let mut g = Generator::new(
            GeneratorConfig {
                signal: Signal::Sweep {
                    from_hz: 100.0,
                    to_hz: 8000.0,
                    seconds: 2.0,
                },
                level_dbfs: -12.0,
                ..Default::default()
            },
            RATE,
        );
        // Look at the start and the near-end of one pass. The skip has to leave
        // the late window inside the same pass: the sweep repeats, so skipping
        // past 2 s wraps back to 100 Hz and the test would read the beginning
        // again while appearing to measure the end.
        let mut early = vec![0.0f32; 16384];
        g.fill(&mut early);
        let mut skip = vec![0.0f32; (RATE * 1.2) as usize];
        g.fill(&mut skip);
        let mut late = vec![0.0f32; 16384];
        g.fill(&mut late);

        let f_early = analyse(&early, Fraction::Third, 8192).dominant_hz().unwrap();
        let f_late = analyse(&late, Fraction::Third, 8192).dominant_hz().unwrap();
        assert!(f_early < 400.0, "sweep started at {f_early:.0} Hz");
        assert!(f_late > 3000.0, "sweep ended at {f_late:.0} Hz");
    }

    #[test]
    fn off_is_silent() {
        let out = generate(GeneratorConfig::default(), 0.5);
        assert!(out.iter().all(|&x| x == 0.0));
    }

    #[test]
    fn interleaved_output_writes_one_channel_and_clears_the_rest() {
        let mut g = Generator::new(
            GeneratorConfig {
                signal: Signal::Sine { hz: 1000.0 },
                level_dbfs: -6.0,
                ..Default::default()
            },
            RATE,
        );
        let mut buf = vec![9.0f32; 4 * 100];
        g.fill_interleaved(&mut buf, 4, 2);
        for f in 0..100 {
            for c in 0..4 {
                let v = buf[f * 4 + c];
                if c == 2 {
                    assert!(v.abs() <= 1.0);
                } else {
                    assert_eq!(v, 0.0, "channel {c} was not cleared");
                }
            }
        }
    }

    #[test]
    fn an_out_of_range_channel_falls_back_to_the_last_one() {
        let mut g = Generator::new(
            GeneratorConfig {
                signal: Signal::Sine { hz: 1000.0 },
                level_dbfs: -6.0,
                ..Default::default()
            },
            RATE,
        );
        let mut buf = vec![0.0f32; 2 * 64];
        g.fill_interleaved(&mut buf, 2, 9);
        let last_has_signal = (0..64).any(|f| buf[f * 2 + 1] != 0.0);
        assert!(last_has_signal);
    }

    #[test]
    fn band_limiting_removes_what_it_says_it_does() {
        let out = generate(
            GeneratorConfig {
                signal: Signal::Pink,
                level_dbfs: -20.0,
                high_pass_hz: Some(500.0),
                low_pass_hz: Some(2000.0),
            },
            10.0,
        );
        let a = analyse(&out, Fraction::Third, 8192);
        let at = |hz: f64| {
            a.plan()
                .bands
                .iter()
                .position(|b| b.flo <= hz && hz < b.fhi)
                .map(|i| a.bands_db()[i] as f64)
                .unwrap()
        };
        let passband = at(1000.0);
        assert!(
            at(100.0) < passband - 8.0,
            "100 Hz was only {:.1} dB down",
            passband - at(100.0)
        );
        assert!(
            at(10000.0) < passband - 8.0,
            "10 kHz was only {:.1} dB down",
            passband - at(10000.0)
        );
    }

    #[test]
    fn reseeding_makes_a_measurement_repeatable() {
        let config = GeneratorConfig {
            signal: Signal::Pink,
            level_dbfs: -20.0,
            ..Default::default()
        };
        let mut a = Generator::new(config, RATE);
        let mut b = Generator::new(config, RATE);
        a.reseed(42);
        b.reseed(42);
        let mut x = vec![0.0f32; 4096];
        let mut y = vec![0.0f32; 4096];
        a.fill(&mut x);
        b.fill(&mut y);
        assert_eq!(x, y);

        b.reseed(43);
        let mut z = vec![0.0f32; 4096];
        b.fill(&mut z);
        assert_ne!(x, z, "two different seeds gave the same noise");
    }

    #[test]
    fn a_long_run_does_not_drift_or_blow_up() {
        // Ten minutes of sine, checking the phase accumulator stays sane and
        // the level does not creep.
        let mut g = Generator::new(
            GeneratorConfig {
                signal: Signal::Sine { hz: 997.0 },
                level_dbfs: -12.0,
                ..Default::default()
            },
            RATE,
        );
        let mut buf = vec![0.0f32; (RATE * 10.0) as usize];
        // Discard the first block: it contains the 20 ms start ramp, so its RMS
        // is legitimately a shade lower and comparing against it would flag a
        // drift that is really the fade-in.
        g.fill(&mut buf);

        let mut last_rms = 0.0;
        for block in 0..60 {
            g.fill(&mut buf);
            let rms = (buf.iter().map(|&x| (x as f64) * (x as f64)).sum::<f64>()
                / buf.len() as f64)
                .sqrt();
            assert!(rms.is_finite() && rms > 0.0);
            if block > 0 {
                // 997 Hz does not divide the block length into whole cycles, so
                // the RMS of successive blocks differs in the last few digits.
                // Anything past 1e-4 relative is a real drift.
                let relative = (rms - last_rms).abs() / last_rms;
                assert!(
                    relative < 1e-4,
                    "level drifted at block {block}: {last_rms} -> {rms}"
                );
            }
            last_rms = rms;
        }
    }
}
