//! Two channels against each other: the vectorscope, phase correlation and
//! balance.
//!
//! A vectorscope (a goniometer, on the older desks) plots each instant of a
//! stereo pair as one point. It is drawn rotated by 45°, so that the signal
//! everyone checks first — mono — is a vertical line:
//!
//! ```text
//!   x = (R − L) / 2        y = (L + R) / 2
//! ```
//!
//! With that scaling `|x| + |y| = max(|L|, |R|)`, so the full-scale boundary is
//! a diamond: a point outside it means a sample above full scale, and the
//! diamond is exactly what the display draws as its edge. The more usual
//! `1/√2` rotation puts in-phase full scale at √2 and leaves the edge of the
//! plot meaning nothing in particular.
//!
//! ## Correlation is a ratio of averages
//!
//! ```text
//!   r = E[L·R] / √(E[L²] · E[R²])
//! ```
//!
//! Each expectation is its own one-pole average and the ratio is taken last.
//! Averaging per-block correlations instead would weight a quiet block exactly
//! as heavily as a loud one, which is the correlation meter's version of
//! averaging decibels (§4.1): a fade-out would drag the reading about while
//! contributing almost nothing that can be heard. +1 is mono, 0 is two
//! unrelated signals, −1 is one channel inverted — the reading that means the
//! mono fold-down cancels.
//!
//! Correlation and balance are only reported while both sides carry signal.
//! With one side silent the ratio is 0/0, and any number put there would be
//! invented; [`StereoSignal`] says which side is missing instead.
//!
//! ## Points are a ring with a cursor
//!
//! The display pulls points with the cursor it was given last time, so every
//! point is drawn once — what a phosphor display needs to look like the
//! signal — and a display that falls behind is told how many it missed rather
//! than being handed a gap it cannot see. Points are decimated to about
//! [`TARGET_POINT_RATE`] a second. Each kept point is still a true
//! simultaneous L/R pair, so the shape of the cloud is unchanged; only its
//! density is.

use serde::{Deserialize, Serialize};

use crate::spl::mean_square_to_dbfs;

/// Points kept for the display. A few hundred milliseconds at any rate.
pub const SCOPE_CAPACITY: usize = 16_384;

/// Points per second the display is offered.
///
/// Enough for a dense cloud at a 30 Hz repaint, and few enough to cross to the
/// UI without the scope costing more than the RTA does.
pub const TARGET_POINT_RATE: f64 = 24_000.0;

/// Most points one call hands back.
pub const MAX_POINTS_PER_VIEW: usize = 4096;

/// Time constant of the correlation and balance averages, in seconds.
///
/// A choice rather than a standard: fast enough to show a phase problem inside
/// a bar of music, slow enough to read the number.
pub const CORRELATION_TAU: f64 = 0.3;

/// Release of the peak envelope the display scales itself against, in seconds.
pub const PEAK_RELEASE_TAU: f64 = 1.0;

/// Mean square below which a side counts as carrying no signal: −100 dBFS.
///
/// Well below a converter's own noise with the gain at its lowest, so a live
/// but quiet input always counts, and far above the denormal-sized values an
/// average decays to once its input has gone to digital silence.
pub const SIGNAL_FLOOR: f64 = 1e-10;

/// The two inputs the vectorscope reads, counted from zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StereoPair {
    pub left: usize,
    pub right: usize,
}

impl Default for StereoPair {
    fn default() -> Self {
        StereoPair { left: 0, right: 1 }
    }
}

impl StereoPair {
    /// The pair as it will actually be read from a device with `channels`
    /// inputs. Out-of-range channels fall back to the last one, as
    /// [`crate::engine::ChannelSelect`] does — on a mono device both sides
    /// become input 1, and the reading says so.
    pub fn resolve(self, channels: usize) -> StereoPair {
        if channels == 0 {
            return self;
        }
        StereoPair {
            left: self.left.min(channels - 1),
            right: self.right.min(channels - 1),
        }
    }
}

/// Which sides of the pair are carrying signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum StereoSignal {
    None,
    LeftOnly,
    RightOnly,
    Both,
}

/// The stereo readout for one repaint.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StereoReading {
    /// The inputs actually read, after falling back for a short device.
    pub left: usize,
    pub right: usize,
    pub signal: StereoSignal,
    /// −1 to +1. `None` unless both sides carry signal.
    pub correlation: Option<f64>,
    /// Lowest correlation since the last reset: whether the mono fold-down
    /// ever cancelled, which a live needle cannot tell you after the fact.
    pub correlation_min: Option<f64>,
    /// Left level relative to right, dB. Positive means left is louder.
    pub balance_db: Option<f64>,
    /// Peak envelope of `max(|L|, |R|)`, linear, full scale = 1. What the
    /// display scales itself against; never presented as a level.
    pub peak: f64,
}

/// Points for the display, and where to ask from next time.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScopePoints {
    /// Interleaved `x, y` in the rotated, diamond-bounded coordinates above.
    pub points: Vec<f32>,
    /// The cursor to pass next time.
    pub next: u64,
    /// Points that existed but are not here, because the caller fell further
    /// behind than the ring or the per-call limit allows.
    pub skipped: u64,
}

/// Vectorscope ring, correlation and balance for one pair of inputs.
pub struct Stereo {
    pair: StereoPair,
    /// The pair as last resolved against a real channel count.
    resolved: StereoPair,
    stride: usize,
    /// Samples until the next one is kept.
    countdown: usize,
    ring: Vec<[f32; 2]>,
    /// Points ever written. The ring holds the last `SCOPE_CAPACITY` of them.
    written: u64,
    alpha: f64,
    lr: f64,
    ll: f64,
    rr: f64,
    release_per_sample: f64,
    peak: f64,
    correlation_min: Option<f64>,
}

impl Stereo {
    pub fn new(pair: StereoPair, sample_rate: f64) -> Self {
        let mut s = Stereo {
            pair,
            resolved: pair,
            stride: 1,
            countdown: 0,
            ring: vec![[0.0, 0.0]; SCOPE_CAPACITY],
            written: 0,
            alpha: 0.0,
            lr: 0.0,
            ll: 0.0,
            rr: 0.0,
            release_per_sample: 0.0,
            peak: 0.0,
            correlation_min: None,
        };
        s.set_rate(sample_rate);
        s
    }

    fn set_rate(&mut self, sample_rate: f64) {
        let rate = sample_rate.max(1.0);
        self.stride = ((rate / TARGET_POINT_RATE).round() as usize).max(1);
        self.countdown = self.countdown.min(self.stride - 1);
        self.alpha = 1.0 - (-1.0 / (CORRELATION_TAU * rate)).exp();
        self.release_per_sample = (-1.0 / (PEAK_RELEASE_TAU * rate)).exp();
    }

    pub fn pair(&self) -> StereoPair {
        self.pair
    }

    /// Change the pair or the rate.
    ///
    /// A new pair throws away the averages and the held minimum: they describe
    /// two other inputs, and a correlation that slid from the old pair's value to
    /// the new one's over the next second would be a reading of neither. The
    /// points already in the ring are kept, since they were drawn honestly.
    pub fn reconfigure(&mut self, pair: StereoPair, sample_rate: f64) {
        if pair != self.pair {
            self.pair = pair;
            self.clear_averages();
        }
        self.set_rate(sample_rate);
    }

    fn clear_averages(&mut self) {
        self.lr = 0.0;
        self.ll = 0.0;
        self.rr = 0.0;
        self.peak = 0.0;
        self.correlation_min = None;
    }

    /// Feed an interleaved block.
    pub fn push_interleaved(&mut self, samples: &[f32], channels: usize) {
        if channels == 0 {
            return;
        }
        let resolved = self.pair.resolve(channels);
        if resolved != self.resolved {
            // The device changed under an unchanged setting — a mono device
            // swapped for a stereo one. Same reasoning as a new pair.
            self.resolved = resolved;
            self.clear_averages();
        }
        let (li, ri) = (resolved.left, resolved.right);
        let frames = samples.len() / channels;
        let a = self.alpha;
        let mut block_peak = 0.0f64;

        for f in 0..frames {
            let l = samples[f * channels + li];
            let r = samples[f * channels + ri];
            let (lf, rf) = (l as f64, r as f64);
            self.lr += a * (lf * rf - self.lr);
            self.ll += a * (lf * lf - self.ll);
            self.rr += a * (rf * rf - self.rr);
            block_peak = block_peak.max(lf.abs()).max(rf.abs());

            if self.countdown == 0 {
                let i = (self.written % SCOPE_CAPACITY as u64) as usize;
                self.ring[i] = [(r - l) * 0.5, (l + r) * 0.5];
                self.written += 1;
                self.countdown = self.stride - 1;
            } else {
                self.countdown -= 1;
            }
        }

        // A block-wise release is the per-sample one applied `frames` times,
        // so the envelope does not depend on how the audio was chunked.
        let decayed = self.peak * self.release_per_sample.powi(frames as i32);
        self.peak = decayed.max(block_peak);

        if let Some(r) = self.correlation() {
            self.correlation_min = Some(self.correlation_min.map_or(r, |m| m.min(r)));
        }
    }

    pub fn signal(&self) -> StereoSignal {
        match (self.ll > SIGNAL_FLOOR, self.rr > SIGNAL_FLOOR) {
            (true, true) => StereoSignal::Both,
            (true, false) => StereoSignal::LeftOnly,
            (false, true) => StereoSignal::RightOnly,
            (false, false) => StereoSignal::None,
        }
    }

    pub fn correlation(&self) -> Option<f64> {
        if self.signal() != StereoSignal::Both {
            return None;
        }
        // Clamped because rounding in three separate averages can put the ratio
        // a hair outside ±1 on a perfectly mono signal, and "1.0000000002" is a
        // reading no correlation can have.
        Some((self.lr / (self.ll * self.rr).sqrt()).clamp(-1.0, 1.0))
    }

    pub fn balance_db(&self) -> Option<f64> {
        if self.signal() != StereoSignal::Both {
            return None;
        }
        Some(mean_square_to_dbfs(self.ll) - mean_square_to_dbfs(self.rr))
    }

    pub fn reading(&self) -> StereoReading {
        StereoReading {
            left: self.resolved.left,
            right: self.resolved.right,
            signal: self.signal(),
            correlation: self.correlation(),
            correlation_min: self.correlation_min,
            balance_db: self.balance_db(),
            peak: self.peak,
        }
    }

    /// Points written since `since`, at most `max` of them (and never more than
    /// [`MAX_POINTS_PER_VIEW`]).
    ///
    /// A cursor from the future — one handed out before the engine was rebuilt
    /// — is treated as a fresh start rather than an error: the display just
    /// gets the most recent points.
    pub fn points_since(&self, since: u64, max: usize) -> ScopePoints {
        let max = max.clamp(1, MAX_POINTS_PER_VIEW) as u64;
        let oldest = self.written.saturating_sub(SCOPE_CAPACITY as u64);
        let since = if since > self.written {
            self.written.saturating_sub(max)
        } else {
            since
        };
        let start = since.max(oldest).max(self.written.saturating_sub(max));
        let mut points = Vec::with_capacity(((self.written - start) * 2) as usize);
        for n in start..self.written {
            let p = self.ring[(n % SCOPE_CAPACITY as u64) as usize];
            points.push(p[0]);
            points.push(p[1]);
        }
        ScopePoints {
            points,
            next: self.written,
            skipped: start - since,
        }
    }

    /// Clear the held minimum. The live averages carry on: they describe the
    /// last few hundred milliseconds, not the measurement.
    pub fn reset_statistics(&mut self) {
        self.correlation_min = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    const RATE: f64 = 48000.0;

    /// Interleave a stereo pair from two generators.
    fn stereo(seconds: f64, l: impl Fn(f64) -> f64, r: impl Fn(f64) -> f64) -> Vec<f32> {
        let frames = (seconds * RATE) as usize;
        let mut v = Vec::with_capacity(frames * 2);
        for i in 0..frames {
            let t = i as f64 / RATE;
            v.push(l(t) as f32);
            v.push(r(t) as f32);
        }
        v
    }

    fn sine(hz: f64, phase: f64) -> impl Fn(f64) -> f64 {
        move |t| 0.5 * (2.0 * PI * hz * t + phase).sin()
    }

    /// A deterministic white noise, so a failure reproduces: SplitMix64 over
    /// the sample index, one stream per seed.
    fn noise(seed: u64) -> impl Fn(f64) -> f64 {
        move |t| {
            let mut z = ((((t * RATE).round() as u64) << 8) | seed)
                .wrapping_add(0x9E37_79B9_7F4A_7C15);
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^= z >> 31;
            (z as f64 / u64::MAX as f64) - 0.5
        }
    }

    fn run(samples: &[f32]) -> Stereo {
        let mut s = Stereo::new(StereoPair::default(), RATE);
        s.push_interleaved(samples, 2);
        s
    }

    #[test]
    fn mono_reads_plus_one_and_inverted_reads_minus_one() {
        let s = run(&stereo(1.0, sine(1000.0, 0.0), sine(1000.0, 0.0)));
        assert!((s.correlation().unwrap() - 1.0).abs() < 1e-9);

        let s = run(&stereo(1.0, sine(1000.0, 0.0), sine(1000.0, PI)));
        assert!((s.correlation().unwrap() + 1.0).abs() < 1e-9);
    }

    /// Correlation of two sines is the cosine of the phase between them. At 90°
    /// they are uncorrelated, which is the case a meter that only checked
    /// polarity would get wrong.
    #[test]
    fn correlation_follows_the_cosine_of_the_phase_difference() {
        for deg in [0.0f64, 30.0, 60.0, 90.0, 120.0, 180.0] {
            let s = run(&stereo(2.0, sine(997.0, 0.0), sine(997.0, deg.to_radians())));
            let r = s.correlation().unwrap();
            assert!(
                (r - deg.to_radians().cos()).abs() < 0.01,
                "{deg}°: correlation {r}, expected {}",
                deg.to_radians().cos()
            );
        }
    }

    #[test]
    fn unrelated_noise_reads_near_zero() {
        let s = run(&stereo(3.0, noise(1), noise(2)));
        let r = s.correlation().unwrap();
        assert!(r.abs() < 0.05, "independent noise read {r}");
    }

    /// Correlation is about shape, not level: a quiet copy of the left channel
    /// is still perfectly correlated with it.
    #[test]
    fn correlation_ignores_level_and_balance_measures_it() {
        let s = run(&stereo(1.0, sine(500.0, 0.0), |t| 0.1 * sine(500.0, 0.0)(t)));
        assert!((s.correlation().unwrap() - 1.0).abs() < 1e-9);
        let b = s.balance_db().unwrap();
        assert!((b - 20.0).abs() < 0.05, "a tenth the amplitude is 20 dB down, got {b}");
    }

    #[test]
    fn one_silent_side_reports_which_rather_than_a_number() {
        let s = run(&stereo(1.0, sine(1000.0, 0.0), |_| 0.0));
        assert_eq!(s.signal(), StereoSignal::LeftOnly);
        assert!(s.correlation().is_none());
        assert!(s.balance_db().is_none());

        let s = run(&stereo(1.0, |_| 0.0, |_| 0.0));
        assert_eq!(s.signal(), StereoSignal::None);
        assert!(s.reading().correlation_min.is_none());
    }

    /// The §4.1 property, for correlation: the reading is a ratio of energy
    /// averages, so a long quiet stretch barely moves it. Averaging the
    /// correlation of each block would let the quiet part count for as much as
    /// the loud part and read near zero here.
    #[test]
    fn a_quiet_stretch_does_not_outvote_a_loud_one() {
        let mut s = Stereo::new(StereoPair::default(), RATE);
        // Loud and mono, then very quiet and inverted, for the same length of
        // time as one time constant.
        s.push_interleaved(&stereo(1.0, sine(800.0, 0.0), sine(800.0, 0.0)), 2);
        let quiet = |t: f64| 0.003 * sine(800.0, 0.0)(t);
        let quiet_inv = |t: f64| -0.003 * sine(800.0, 0.0)(t);
        s.push_interleaved(&stereo(CORRELATION_TAU, quiet, quiet_inv), 2);
        let r = s.correlation().unwrap();
        assert!(r > 0.99, "the loud mono part should still dominate, got {r}");
    }

    #[test]
    fn the_minimum_is_held_until_reset() {
        let mut s = Stereo::new(StereoPair::default(), RATE);
        s.push_interleaved(&stereo(1.0, sine(1000.0, 0.0), sine(1000.0, PI)), 2);
        s.push_interleaved(&stereo(2.0, sine(1000.0, 0.0), sine(1000.0, 0.0)), 2);
        assert!(s.correlation().unwrap() > 0.99);
        assert!(s.reading().correlation_min.unwrap() < -0.99);

        s.reset_statistics();
        s.push_interleaved(&stereo(0.1, sine(1000.0, 0.0), sine(1000.0, 0.0)), 2);
        assert!(s.reading().correlation_min.unwrap() > 0.99);
    }

    /// Mono is vertical, a single side lies on its diagonal, and the diamond
    /// |x| + |y| = 1 is exactly full scale.
    #[test]
    fn points_are_rotated_so_mono_is_vertical() {
        let mut s = Stereo::new(StereoPair::default(), RATE);
        s.push_interleaved(&[0.5, 0.5, 1.0, 0.0, 0.0, 1.0, 0.4, -0.4], 2);
        // Stride 2 at 48 kHz keeps samples 0 and 2.
        let p = s.points_since(0, 100).points;
        assert_eq!(p, vec![0.0, 0.5, 0.5, 0.5]);

        let mut s = Stereo::new(StereoPair::default(), 24000.0);
        s.push_interleaved(&[1.0, 0.0, 0.0, 1.0, 0.4, -0.4, -1.0, -1.0], 2);
        let p = s.points_since(0, 100).points;
        // Left only: upper left. Right only: upper right. Out of phase:
        // horizontal. Both at full scale: the bottom vertex.
        assert_eq!(p, vec![-0.5, 0.5, 0.5, 0.5, -0.4, 0.0, 0.0, -1.0]);
        for xy in p.chunks(2) {
            assert!(xy[0].abs() + xy[1].abs() <= 1.0 + 1e-6);
        }
    }

    #[test]
    fn decimation_follows_the_sample_rate() {
        for (rate, stride) in [(44100.0, 2), (48000.0, 2), (96000.0, 4), (192000.0, 8), (16000.0, 1)] {
            let s = Stereo::new(StereoPair::default(), rate);
            assert_eq!(s.stride, stride, "at {rate} Hz");
        }
    }

    /// A display that keeps up sees every point exactly once.
    #[test]
    fn a_cursor_sees_each_point_once() {
        let mut s = Stereo::new(StereoPair::default(), 24000.0);
        let mut cursor = 0;
        let mut seen = 0;
        for _ in 0..10 {
            s.push_interleaved(&stereo(0.01, sine(100.0, 0.0), sine(100.0, 0.0)), 2);
            let v = s.points_since(cursor, MAX_POINTS_PER_VIEW);
            assert_eq!(v.skipped, 0);
            seen += v.points.len() / 2;
            cursor = v.next;
        }
        assert_eq!(seen as u64, s.written);
    }

    #[test]
    fn a_display_that_falls_behind_is_told_how_far() {
        let mut s = Stereo::new(StereoPair::default(), 24000.0);
        s.push_interleaved(&vec![0.1f32; 2 * (SCOPE_CAPACITY + 1000)], 2);
        let v = s.points_since(0, MAX_POINTS_PER_VIEW);
        assert_eq!(v.points.len() / 2, MAX_POINTS_PER_VIEW);
        assert_eq!(v.skipped, s.written - MAX_POINTS_PER_VIEW as u64);

        // A cursor from before a rebuild just gets the latest points.
        let v = s.points_since(u64::MAX, 10);
        assert_eq!(v.points.len(), 20);
        assert_eq!(v.next, s.written);
    }

    #[test]
    fn a_mono_device_reads_one_input_on_both_sides() {
        let mut s = Stereo::new(StereoPair::default(), RATE);
        let mono: Vec<f32> = (0..48000)
            .map(|i| sine(1000.0, 0.0)(i as f64 / RATE) as f32)
            .collect();
        s.push_interleaved(&mono, 1);
        let r = s.reading();
        assert_eq!((r.left, r.right), (0, 0));
        assert!((r.correlation.unwrap() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn the_pair_reads_the_inputs_it_names() {
        // Four inputs: 0 and 1 silent, 2 and 3 an inverted pair.
        let frames = 24000;
        let mut buf = Vec::with_capacity(frames * 4);
        for i in 0..frames {
            let v = sine(440.0, 0.0)(i as f64 / RATE) as f32;
            buf.extend_from_slice(&[0.0, 0.0, v, -v]);
        }
        let mut s = Stereo::new(StereoPair { left: 2, right: 3 }, RATE);
        s.push_interleaved(&buf, 4);
        assert!((s.correlation().unwrap() + 1.0).abs() < 1e-9);
    }

    #[test]
    fn the_peak_envelope_holds_then_releases() {
        let mut s = Stereo::new(StereoPair::default(), RATE);
        s.push_interleaved(&[0.8, -0.2], 2);
        assert!((s.peak - 0.8).abs() < 1e-6);
        // One release time constant later it has fallen to 1/e.
        s.push_interleaved(&vec![0.0f32; 2 * RATE as usize], 2);
        assert!((s.peak - 0.8 / std::f64::consts::E).abs() < 1e-3, "{}", s.peak);
    }

    #[test]
    fn the_wire_format_is_camel_case() {
        let s = run(&stereo(0.5, sine(1000.0, 0.0), sine(1000.0, 0.3)));
        let v = serde_json::to_value(s.reading()).unwrap();
        for key in ["left", "right", "signal", "correlation", "correlationMin", "balanceDb", "peak"] {
            assert!(v.get(key).is_some(), "{key} missing from {v}");
        }
        assert_eq!(v["signal"], "both");
        let p = serde_json::to_value(s.points_since(0, 4)).unwrap();
        for key in ["points", "next", "skipped"] {
            assert!(p.get(key).is_some(), "{key} missing from {p}");
        }
        let pair = serde_json::to_value(StereoPair::default()).unwrap();
        assert_eq!(pair, serde_json::json!({ "left": 0, "right": 1 }));
    }
}
