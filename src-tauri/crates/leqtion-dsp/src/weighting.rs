//! Frequency weighting, IEC 61672-1.
//!
//! Two forms of the same thing, and the distinction is the reason this file is
//! not four lines long:
//!
//! - [`Weighting::curve_db`] is the **analytic curve**, evaluated at a
//!   frequency. It is what the RTA display uses to tilt a spectrum, and what
//!   the tolerance tests compare against. It has no state and no sample rate.
//! - [`WeightingFilter`] is a **time-domain filter**, a cascade of biquads
//!   fitted to the analogue design. It is what SPL and LEQ run through.
//!
//! LEQ must be filtered in the time domain, not weighted band-by-band after an
//! FFT. LEQ is an integral of weighted pressure squared over time; deriving it
//! from a windowed, overlapped transform makes the answer depend on the window,
//! the overlap and the transform length, none of which have anything to do with
//! the sound. Filtering the samples and then integrating gives an answer that
//! depends only on the signal.
//!
//! ## How the digital filter is built, and why not just bilinear
//!
//! The analogue design has two more poles than zeros, and every transform from
//! s to z has to do *something* with that surplus. The textbook choices are both
//! wrong at the top of the audio band, and not slightly:
//!
//! | Design | A-weighting error at 19 kHz, 48 kHz sample rate |
//! |---|---|
//! | Bilinear (double zero forced onto Nyquist) | −13.6 dB |
//! | Plain matched-Z (no surplus zero at all) | +7.2 dB |
//! | What this module does | **under 1 dB** |
//!
//! The approach here is per-section: the sections carrying the numerator's
//! zeros at the origin are bilinear, because bilinear maps s=0 to z=1 and puts
//! those zeros exactly where they belong. The pole-only sections are matched-Z
//! with a *single* shaping zero at `z = -hf_zero`, and `hf_zero` is fitted by a
//! golden-section search that minimises the worst deviation from the analytic
//! curve over 20 Hz to min(20 kHz, 0.45·fs). The search runs once when a device
//! is opened and costs well under a millisecond.
//!
//! The filter is separately normalised to exactly 0 dB at 1 kHz — digitally,
//! after the transform — which is where IEC 61672 pins it and where a 1 kHz
//! calibrator is measured.
//!
//! ## What it achieves
//!
//! Worst deviation from the analytic curve, measured by
//! `filter_accuracy_is_pinned_per_sample_rate` and
//! `filter_is_tight_where_the_energy_is`:
//!
//! | Sample rate | A, 20 Hz–20 kHz | C, 20 Hz–20 kHz | A, below 5 kHz |
//! |---|---|---|---|
//! | 44.1 kHz | 1.15 dB | 0.30 dB | 0.42 dB |
//! | 48 kHz | 0.99 dB | 0.21 dB | under 0.4 dB |
//! | 96 kHz | 0.22 dB | 0.01 dB | under 0.2 dB |
//!
//! A minimax fit spreads its error rather than confining it to the top octave,
//! so at 44.1 kHz A-weighting is already about 1.1 dB out by 10 kHz. That is the
//! honest shape of it. What it costs in practice is much less: on a deliberately
//! harsh 29-tone signal with as much energy at 16 kHz as at 1 kHz, the resulting
//! A-weighted level is 0.23 dB out, and real programme material falls away at the
//! top where the filter is weakest.
//!
//! Running at 96 kHz makes the weighting effectively exact and costs nothing but
//! CPU, which is why the app reports the sample rate rather than hiding it.
//!
//! **LEQtion is not a certified sound level meter** and does not claim Class 1
//! or Class 2 conformance — that is a statement about a calibrated instrument
//! and its microphone, not about a filter. These figures are published so the
//! filter can be judged rather than trusted; use
//! [`WeightingFilter::deviation_from_curve_db`] to ask it directly at any
//! frequency.

use serde::{Deserialize, Serialize};

/// Pole frequencies from IEC 61672-1, in Hz.
const F1: f64 = 20.598_997;
const F2: f64 = 107.652_65;
const F3: f64 = 737.862_23;
const F4: f64 = 12_194.217;

/// The reference frequency at which every weighting is 0 dB by definition.
pub const REFERENCE_HZ: f64 = 1000.0;

/// Search bounds for the surplus zero's position. 0 is plain matched-Z (no
/// zero), 1 is a zero exactly at Nyquist, and past 1 the zero moves outside the
/// unit circle — legal for a zero, and sometimes the better fit.
const HF_ZERO_RANGE: (f64, f64) = (0.0, 2.0);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Weighting {
    /// A-weighting. The one every noise limit is written in.
    A,
    /// C-weighting. Flat through the middle, rolled off at the extremes.
    C,
    /// Z-weighting: no weighting at all (IEC 61672 calls this "zero frequency
    /// weighting"). Not the same as "linear" on an older meter, which usually
    /// meant some unstated bandpass.
    Z,
}

impl Weighting {
    pub const ALL: [Weighting; 3] = [Weighting::A, Weighting::C, Weighting::Z];

    pub fn label(self) -> &'static str {
        match self {
            Weighting::A => "A",
            Weighting::C => "C",
            Weighting::Z => "Z",
        }
    }

    /// Weighting in dB at a frequency, from the analytic transfer function,
    /// normalised to 0 dB at 1 kHz.
    pub fn curve_db(self, f: f64) -> f64 {
        weighted_curve_db(self, f)
    }
}

/// Unnormalised curve magnitudes, used to derive the 1 kHz offsets.
fn raw_curve_db(w: Weighting, f: f64) -> f64 {
    let f2 = f * f;
    match w {
        Weighting::Z => 0.0,
        Weighting::A => {
            let num = F4 * F4 * f2 * f2;
            let den = (f2 + F1 * F1) * ((f2 + F2 * F2) * (f2 + F3 * F3)).sqrt() * (f2 + F4 * F4);
            20.0 * (num / den).log10()
        }
        Weighting::C => {
            let num = F4 * F4 * f2;
            let den = (f2 + F1 * F1) * (f2 + F4 * F4);
            20.0 * (num / den).log10()
        }
    }
}

/// dB to add to the raw curve so it reads exactly 0 dB at 1 kHz.
///
/// This is the +2.0 dB (A) and +0.06 dB (C) usually seen written into the
/// formula as a literal. Computing it instead means `curve_db(1000.0)` is zero
/// to machine precision rather than to two decimal places — worth having,
/// because every weighted level in the app is referenced to that point and a
/// calibration done against a 1 kHz calibrator inherits the error directly.
pub fn normalisation_db(w: Weighting) -> f64 {
    -raw_curve_db(w, REFERENCE_HZ)
}

/// One second-order section, direct form II transposed.
///
/// Transposed DF2 rather than DF1 because it needs two state words instead of
/// four and is the better-behaved of the two in floating point when sections
/// are cascaded — which they always are here.
#[derive(Debug, Clone, Copy, Default)]
pub struct Biquad {
    pub b0: f64,
    pub b1: f64,
    pub b2: f64,
    pub a1: f64,
    pub a2: f64,
    z1: f64,
    z2: f64,
}

impl Biquad {
    /// Bilinear-transform an analogue section `(n2 s² + n1 s + n0) / (d2 s² + d1 s + d0)`,
    /// pre-warped so that `warp_hz` maps to itself.
    ///
    /// The plain substitution uses `c = 2·fs`, which compresses the whole
    /// frequency axis. Choosing `c = ω₀/tan(ω₀T/2)` instead puts one chosen
    /// frequency exactly where it belongs, at the cost of stretching everything
    /// else a little further.
    ///
    /// **Every caller currently passes `None`,** and that is a measured choice
    /// rather than an oversight. Pre-warping each section at its own corner
    /// looks like the obvious improvement — A-weighting's 12.2 kHz double pole
    /// is a quarter of the sample rate at 48 kHz — but it interacts badly with
    /// the fitted surplus zero, and made the broadband A-weighted error nearly
    /// three times worse (0.23 dB → 0.65 dB) when tried. The option is kept
    /// because it is the first thing anyone revisiting this will reach for, and
    /// this note is the answer.
    pub fn bilinear_at(
        n: [f64; 3],
        d: [f64; 3],
        warp_hz: Option<f64>,
        sample_rate: f64,
    ) -> Self {
        let c = match warp_hz {
            Some(hz) if hz > 0.0 && hz < sample_rate * 0.5 => {
                let w0 = 2.0 * std::f64::consts::PI * hz;
                w0 / (w0 / (2.0 * sample_rate)).tan()
            }
            _ => 2.0 * sample_rate,
        };
        let cc = c * c;

        let b0 = n[0] * cc + n[1] * c + n[2];
        let b1 = 2.0 * (n[2] - n[0] * cc);
        let b2 = n[0] * cc - n[1] * c + n[2];

        let a0 = d[0] * cc + d[1] * c + d[2];
        let a1 = 2.0 * (d[2] - d[0] * cc);
        let a2 = d[0] * cc - d[1] * c + d[2];

        Biquad {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
            z1: 0.0,
            z2: 0.0,
        }
    }

    /// Matched-Z for a section with poles only: `1 / ((s+wa)(s+wb))`, with the
    /// numerator shaped by `hf_zero`.
    ///
    /// Each analogue pole at `-w` becomes a digital pole at `e^{-wT}`. The
    /// numerator is `1 + hf_zero·z⁻¹`, which places a single zero at
    /// `z = -hf_zero`; `0.0` is the plain matched-Z form with no zero at all.
    ///
    /// `hf_zero` exists because neither textbook transform gets the top octave
    /// right on its own. The analogue section has two more poles than zeros, so:
    ///
    /// - **Bilinear** maps `s=∞` to `z=−1` and manufactures a *double* zero at
    ///   Nyquist. The filter then rolls off far faster than the curve —
    ///   −13.6 dB at 19 kHz for A-weighting at 48 kHz.
    /// - **Plain matched-Z** manufactures none, and the filter rolls off too
    ///   slowly — +7.2 dB at the same point.
    ///
    /// One zero rather than two lands between the two, which is where the right
    /// answer is. The tests measure what it actually achieves.
    pub fn matched_z_poles(wa: f64, wb: f64, hf_zero: f64, sample_rate: f64) -> Self {
        let t = 1.0 / sample_rate;
        let pa = (-wa * t).exp();
        let pb = (-wb * t).exp();
        Biquad {
            b0: 1.0,
            b1: hf_zero,
            b2: 0.0,
            a1: -(pa + pb),
            a2: pa * pb,
            z1: 0.0,
            z2: 0.0,
        }
    }

    #[inline]
    pub fn process(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }

    pub fn reset(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }

    /// Magnitude response at a normalised angular frequency, linear.
    pub fn magnitude_at(&self, omega: f64) -> f64 {
        let (s1, c1) = omega.sin_cos();
        let (s2, c2) = (2.0 * omega).sin_cos();
        let num_re = self.b0 + self.b1 * c1 + self.b2 * c2;
        let num_im = -(self.b1 * s1 + self.b2 * s2);
        let den_re = 1.0 + self.a1 * c1 + self.a2 * c2;
        let den_im = -(self.a1 * s1 + self.a2 * s2);
        ((num_re * num_re + num_im * num_im) / (den_re * den_re + den_im * den_im)).sqrt()
    }

    fn scale_numerator(&mut self, k: f64) {
        self.b0 *= k;
        self.b1 *= k;
        self.b2 *= k;
    }
}

/// A weighting filter: a cascade of biquads, or nothing at all for Z.
#[derive(Debug, Clone)]
pub struct WeightingFilter {
    pub weighting: Weighting,
    pub sample_rate: f64,
    sections: Vec<Biquad>,
    hf_zero: f64,
}

/// Frequencies the fit is judged over: 1/24-octave from 20 Hz to whichever is
/// lower, 20 kHz or a little below Nyquist. There is no point optimising a
/// filter at frequencies the sample rate cannot represent, and including them
/// would drag the fit away from the range that is actually measured.
fn fit_grid(sample_rate: f64) -> Vec<f64> {
    let top = 20_000f64.min(sample_rate * 0.45);
    let mut v = Vec::new();
    let mut f = 20.0;
    while f <= top {
        v.push(f);
        f *= 2f64.powf(1.0 / 24.0);
    }
    v
}

/// Worst deviation from the analytic curve, in dB, over the fit grid.
///
/// This is the *reporting* measure — what the tests and the docs quote. It is
/// not what the fit minimises; see [`fit_error`].
fn worst_error(sections: &[Biquad], weighting: Weighting, sample_rate: f64, grid: &[f64]) -> f64 {
    let mut worst = 0.0f64;
    for &f in grid {
        let omega = 2.0 * std::f64::consts::PI * f / sample_rate;
        let mag: f64 = sections.iter().map(|s| s.magnitude_at(omega)).product();
        let db = 20.0 * mag.max(1e-300).log10();
        worst = worst.max((db - weighted_curve_db(weighting, f)).abs());
    }
    worst
}

// An energy-weighted RMS objective was tried here, on the theory that error at
// 19 kHz matters less because A-weighting has already attenuated it. It does not
// help: it improved the broadband error only marginally (0.229 → 0.203 dB) while
// letting the worst-case deviation grow to 2.5 dB. One free parameter cannot fix
// both ends of the range, and minimax spends it better. Not reinstated.

impl WeightingFilter {
    pub fn new(weighting: Weighting, sample_rate: f64) -> Self {
        let grid = fit_grid(sample_rate);

        // Golden-section search for the zero position that minimises the worst
        // deviation across the measurement range.
        //
        // Fitting rather than fixing a constant, because the best position
        // depends on the sample rate: at 44.1 kHz the surplus zero has to work
        // much harder than at 96 kHz, where Nyquist is far above anything being
        // measured and almost any choice is fine. It is a one-dimensional
        // search over a smooth unimodal curve, it runs once when a device is
        // opened, and it costs well under a millisecond.
        let (mut lo, mut hi) = HF_ZERO_RANGE;
        let phi = (5f64.sqrt() - 1.0) / 2.0;
        let mut c = hi - phi * (hi - lo);
        let mut d = lo + phi * (hi - lo);
        let mut fc = worst_error(
            &build_sections(weighting, sample_rate, c),
            weighting,
            sample_rate,
            &grid,
        );
        let mut fd = worst_error(
            &build_sections(weighting, sample_rate, d),
            weighting,
            sample_rate,
            &grid,
        );
        for _ in 0..60 {
            if fc < fd {
                hi = d;
                d = c;
                fd = fc;
                c = hi - phi * (hi - lo);
                fc = worst_error(
                    &build_sections(weighting, sample_rate, c),
                    weighting,
                    sample_rate,
                    &grid,
                );
            } else {
                lo = c;
                c = d;
                fc = fd;
                d = lo + phi * (hi - lo);
                fd = worst_error(
                    &build_sections(weighting, sample_rate, d),
                    weighting,
                    sample_rate,
                    &grid,
                );
            }
        }
        let hf_zero = 0.5 * (lo + hi);

        WeightingFilter {
            weighting,
            sample_rate,
            sections: build_sections(weighting, sample_rate, hf_zero),
            hf_zero,
        }
    }
}

/// Build the normalised section cascade for a given surplus-zero position.
fn build_sections(weighting: Weighting, sample_rate: f64, hf_zero: f64) -> Vec<Biquad> {
    {
        let w1 = 2.0 * std::f64::consts::PI * F1;
        let w2 = 2.0 * std::f64::consts::PI * F2;
        let w3 = 2.0 * std::f64::consts::PI * F3;
        let w4 = 2.0 * std::f64::consts::PI * F4;

        // The analogue designs, split into second-order sections.
        //
        //   H_A(s) = K · s⁴ / [ (s+w1)² (s+w2)(s+w3) (s+w4)² ]
        //   H_C(s) = K · s² / [ (s+w1)² (s+w4)² ]
        //
        // Two different transforms, chosen per section by what its zeros are:
        //
        //  - Sections that carry the numerator's zeros at the origin are
        //    **bilinear**. Bilinear maps s=0 to z=1, so those zeros land exactly
        //    where they belong and the low-frequency rolloff is right.
        //  - Sections with poles and no zeros are **matched-Z**. Bilinear maps
        //    s=∞ to z=−1, so it would manufacture a double zero at Nyquist that
        //    the analogue design does not have. That single detail costs more
        //    than 13 dB at 19 kHz on A-weighting at 48 kHz — the filter rolls
        //    off far faster than the curve it is supposed to be. Matched-Z
        //    places the poles at e^{-wT} and leaves the numerator constant, so
        //    nothing is invented.
        //
        // K is not written here: the cascade is normalised digitally below, so
        // the filter is 0 dB at 1 kHz *after* the transform rather than before
        // it. Normalising the analogue instead leaves a small offset behind,
        // and an SPL that reads 0.05 dB high at the calibration frequency is
        // exactly the kind of error that never gets found.
        let mut sections: Vec<Biquad> = match weighting {
            Weighting::Z => Vec::new(),
            Weighting::A => vec![
                Biquad::bilinear_at([1.0, 0.0, 0.0], [1.0, 2.0 * w1, w1 * w1], None, sample_rate),
                Biquad::bilinear_at([1.0, 0.0, 0.0], [1.0, 2.0 * w4, w4 * w4], None, sample_rate),
                Biquad::matched_z_poles(w2, w3, hf_zero, sample_rate),
            ],
            Weighting::C => vec![
                Biquad::bilinear_at([1.0, 0.0, 0.0], [1.0, 2.0 * w1, w1 * w1], None, sample_rate),
                Biquad::matched_z_poles(w4, w4, hf_zero, sample_rate),
            ],
        };

        if !sections.is_empty() {
            let omega = 2.0 * std::f64::consts::PI * REFERENCE_HZ / sample_rate;
            let mag: f64 = sections.iter().map(|s| s.magnitude_at(omega)).product();
            if mag > 0.0 {
                sections[0].scale_numerator(1.0 / mag);
            }
        }

        sections
    }
}

impl WeightingFilter {
    /// Where the fit put the surplus zero. Exposed for the tests and for the
    /// diagnostics bundle, not for the UI.
    pub fn hf_zero(&self) -> f64 {
        self.hf_zero
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let mut v = x as f64;
        for s in &mut self.sections {
            v = s.process(v);
        }
        v as f32
    }

    /// Filter a block in place.
    pub fn process_block(&mut self, buf: &mut [f32]) {
        if self.sections.is_empty() {
            return;
        }
        for x in buf.iter_mut() {
            *x = self.process(*x);
        }
    }

    pub fn reset(&mut self) {
        for s in &mut self.sections {
            s.reset();
        }
    }

    /// Magnitude response of the realised digital filter, in dB.
    pub fn response_db(&self, f: f64) -> f64 {
        if self.sections.is_empty() {
            return 0.0;
        }
        let omega = 2.0 * std::f64::consts::PI * f / self.sample_rate;
        let mag: f64 = self.sections.iter().map(|s| s.magnitude_at(omega)).product();
        20.0 * mag.log10()
    }

    /// How far the realised filter sits from the analytic curve, in dB.
    ///
    /// Positive means the filter passes more than the ideal. This is the number
    /// to quote when someone asks whether the meter is Class 1 — see the module
    /// docs for where it stops being small.
    pub fn deviation_from_curve_db(&self, f: f64) -> f64 {
        self.response_db(f) - weighted_curve_db(self.weighting, f)
    }
}

/// The normalised analytic curve, in dB.
pub fn weighted_curve_db(w: Weighting, f: f64) -> f64 {
    if f <= 0.0 {
        return f64::NEG_INFINITY;
    }
    raw_curve_db(w, f) + normalisation_db(w)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// IEC 61672-1 Table 3, as `(band number, A, C)`.
    ///
    /// The band number is the exponent in the ISO 266 base-10 series: the exact
    /// midband frequency is `10^(n/10)`. This distinction is not pedantry — the
    /// table is *labelled* with nominal frequencies (31.5 Hz, 63 Hz) but the
    /// values in it are computed at the exact ones (31.623 Hz, 63.096 Hz).
    /// Evaluating the formula at 31.5 gives −39.53 dB against a published
    /// −39.4, which looks like a broken implementation and is really a broken
    /// test. Columns with no published C value are `None`.
    const IEC_TABLE: &[(i32, f64, Option<f64>)] = &[
        (10, -70.4, Some(-14.3)),
        (13, -50.5, Some(-6.2)),
        (15, -39.4, Some(-3.0)),
        (18, -26.2, Some(-0.8)),
        (21, -16.1, Some(-0.2)),
        (24, -8.6, Some(0.0)),
        (27, -3.2, Some(0.0)),
        (30, 0.0, Some(0.0)),
        (33, 1.2, Some(-0.2)),
        (36, 1.0, Some(-0.8)),
        (39, -1.1, Some(-3.0)),
        (42, -6.6, Some(-8.5)),
        (43, -9.3, Some(-11.2)),
    ];

    /// Exact ISO 266 midband frequency for a band number.
    fn midband_hz(n: i32) -> f64 {
        10f64.powf(n as f64 / 10.0)
    }

    #[test]
    fn analytic_curve_matches_iec_table() {
        // The table is published to 0.1 dB, so half a step — 0.05 dB — is the
        // most that can be asked of it, with a whisker for the standard's own
        // rounding of the intermediate arithmetic.
        const TOL: f64 = 0.06;
        for &(n, a, c) in IEC_TABLE {
            let f = midband_hz(n);
            let got_a = weighted_curve_db(Weighting::A, f);
            assert!(
                (got_a - a).abs() < TOL,
                "A at band {n} ({f:.1} Hz): {got_a:.3} dB, table says {a}"
            );
            if let Some(c) = c {
                let got_c = weighted_curve_db(Weighting::C, f);
                assert!(
                    (got_c - c).abs() < TOL,
                    "C at band {n} ({f:.1} Hz): {got_c:.3} dB, table says {c}"
                );
            }
        }
    }

    #[test]
    fn curves_are_exactly_zero_at_one_kilohertz() {
        for w in [Weighting::A, Weighting::C, Weighting::Z] {
            let got = weighted_curve_db(w, REFERENCE_HZ);
            assert!(got.abs() < 1e-12, "{:?} at 1 kHz: {got}", w);
        }
    }

    #[test]
    fn filter_is_unity_at_one_kilohertz() {
        for rate in [44100.0, 48000.0, 96000.0] {
            for w in [Weighting::A, Weighting::C] {
                let f = WeightingFilter::new(w, rate);
                let got = f.response_db(REFERENCE_HZ);
                assert!(
                    got.abs() < 1e-9,
                    "{w:?} at {rate}: 1 kHz response {got} dB, expected 0"
                );
            }
        }
    }

    /// Worst deviation from the analytic curve at each sample rate.
    ///
    /// These are the figures quoted in the module docs, held to a small margin
    /// so a change to the fit shows up here rather than silently making every
    /// weighted level slightly wrong. If a redesign genuinely improves them,
    /// lower the numbers here *and* in the docs — they are meant to agree.
    #[test]
    fn filter_accuracy_is_pinned_per_sample_rate() {
        let budget = [
            (44100.0, Weighting::A, 1.20),
            (44100.0, Weighting::C, 0.35),
            (48000.0, Weighting::A, 1.05),
            (48000.0, Weighting::C, 0.25),
            (96000.0, Weighting::A, 0.25),
            (96000.0, Weighting::C, 0.05),
        ];
        for (rate, w, limit) in budget {
            let f = WeightingFilter::new(w, rate);
            let top = 20000f64.min(rate * 0.45);
            let mut worst = 0.0f64;
            let mut worst_f = 0.0;
            let mut hz = 20.0f64;
            while hz <= top {
                let d = f.deviation_from_curve_db(hz).abs();
                if d > worst {
                    worst = d;
                    worst_f = hz;
                }
                hz *= 2f64.powf(1.0 / 24.0);
            }
            assert!(
                worst < limit,
                "{w:?} at {rate}: worst deviation {worst:.3} dB at {worst_f:.0} Hz, budget {limit}"
            );
        }
    }

    /// Below 5 kHz the filter has to be much better than the headline figure.
    ///
    /// 5 kHz, not 10 kHz, and that boundary is measured rather than chosen: a
    /// minimax fit spreads its error rather than confining it to the top
    /// octave, and at 44.1 kHz the A-weighting deviation is already 1.12 dB by
    /// 10 kHz. So the guarantee here covers the range that decides a broadband
    /// level, and the 5–20 kHz region is left to
    /// `filter_accuracy_is_pinned_per_sample_rate`. Claiming "tight below
    /// 10 kHz" would have been comfortable and untrue.
    #[test]
    fn filter_is_tight_where_the_energy_is() {
        for rate in [44100.0, 48000.0, 96000.0] {
            for w in [Weighting::A, Weighting::C] {
                let f = WeightingFilter::new(w, rate);
                let mut worst = 0.0f64;
                let mut worst_f = 0.0;
                let mut hz = 20.0f64;
                while hz <= 5000.0 {
                    let d = f.deviation_from_curve_db(hz).abs();
                    if d > worst {
                        worst = d;
                        worst_f = hz;
                    }
                    hz *= 2f64.powf(1.0 / 24.0);
                }
                assert!(
                    worst < 0.6,
                    "{w:?} at {rate}: worst sub-5 kHz deviation {worst:.3} dB at {worst_f:.0} Hz"
                );
            }
        }
    }

    /// The fit must actually find something better than either textbook
    /// transform, or the whole apparatus is not earning its place.
    #[test]
    fn the_fit_beats_both_naive_transforms() {
        let rate = 48000.0;
        let grid = fit_grid(rate);
        let fitted = WeightingFilter::new(Weighting::A, rate);
        let fitted_err = worst_error(&fitted.sections, Weighting::A, rate, &grid);

        // hf_zero = 1 is one zero at Nyquist; 0 is plain matched-Z. Bilinear's
        // double Nyquist zero is worse than either and is covered by the docs.
        let at_nyquist = worst_error(
            &build_sections(Weighting::A, rate, 1.0),
            Weighting::A,
            rate,
            &grid,
        );
        let matched_z = worst_error(
            &build_sections(Weighting::A, rate, 0.0),
            Weighting::A,
            rate,
            &grid,
        );
        assert!(
            fitted_err < at_nyquist && fitted_err < matched_z,
            "fit {fitted_err:.3} dB is no better than {at_nyquist:.3} / {matched_z:.3}"
        );
        assert!(
            (0.0..=2.0).contains(&fitted.hf_zero()),
            "fit escaped its bounds: {}",
            fitted.hf_zero()
        );
    }

    /// The end-to-end claim: a broadband signal filtered in the time domain
    /// must arrive at the same weighted level as the analytic curve predicts.
    ///
    /// This is what actually matters for LAeq. The per-frequency deviation
    /// above is the mechanism; this is the consequence.
    ///
    /// The test signal is deliberately harsher than anything real: 29 equal
    /// sines, one per third-octave, so there is as much energy at 16 kHz as at
    /// 1 kHz. Programme material, room noise and a PA all fall away at the top,
    /// where this filter is weakest, so a real LAeq lands well inside the
    /// 0.3 dB budgeted here. Anything that pushes this number up has changed the
    /// measurement, not just the filter.
    #[test]
    fn a_weighted_level_of_a_broadband_signal_matches_the_curve() {
        let rate = 48000.0;
        // One sine per third-octave from 25 Hz to 16 kHz, equal amplitude.
        let tones: Vec<f64> = (-16..=12).map(|k| 1000.0 * 2f64.powf(k as f64 / 3.0)).collect();
        let amp = 0.02;

        let mut expected_ms = 0.0;
        for &f in &tones {
            expected_ms += 0.5 * amp * amp * 10f64.powf(weighted_curve_db(Weighting::A, f) / 10.0);
        }

        let mut filter = WeightingFilter::new(Weighting::A, rate);
        let n = (rate * 8.0) as usize;
        let settle = (rate * 1.0) as usize;
        let mut got_ms = 0.0;
        let mut counted = 0usize;
        for i in 0..n {
            let t = i as f64 / rate;
            let mut x = 0.0;
            for &f in &tones {
                x += amp * (2.0 * std::f64::consts::PI * f * t).sin();
            }
            let y = filter.process(x as f32) as f64;
            if i >= settle {
                got_ms += y * y;
                counted += 1;
            }
        }
        got_ms /= counted as f64;

        let err = 10.0 * (got_ms / expected_ms).log10();
        assert!(
            err.abs() < 0.3,
            "A-weighted level of a 29-tone signal was off by {err:.4} dB"
        );
    }

    /// A 1 kHz sine through the A filter must come out at the same RMS it went
    /// in with. This is the end-to-end version of the unity test above, and it
    /// is the property calibration depends on.
    #[test]
    fn one_kilohertz_sine_survives_a_weighting() {
        let rate = 48000.0;
        let mut f = WeightingFilter::new(Weighting::A, rate);
        let n = 48000;
        let mut sum_in = 0.0;
        let mut sum_out = 0.0;
        for i in 0..n {
            let x = (2.0 * std::f64::consts::PI * 1000.0 * i as f64 / rate).sin() as f32;
            let y = f.process(x);
            // Skip the first 100 ms so the filter's own transient is not counted.
            if i > 4800 {
                sum_in += (x as f64) * (x as f64);
                sum_out += (y as f64) * (y as f64);
            }
        }
        let db = 10.0 * (sum_out / sum_in).log10();
        assert!(db.abs() < 0.01, "1 kHz sine changed by {db:.4} dB");
    }

    #[test]
    fn z_weighting_is_a_pass_through() {
        let mut f = WeightingFilter::new(Weighting::Z, 48000.0);
        for x in [-1.0f32, -0.25, 0.0, 0.5, 1.0] {
            assert_eq!(f.process(x), x);
        }
        assert_eq!(f.response_db(37.0), 0.0);
    }

    #[test]
    fn filter_is_stable_over_a_long_run() {
        // Cascaded biquads with a pole pair very close to DC (20.6 Hz at 96 kHz
        // is a radius of 0.99865) are the classic place for a slow drift to
        // hide. Run a minute of noise-ish input and check nothing runs away.
        let mut f = WeightingFilter::new(Weighting::A, 96000.0);
        let mut state = 12345u32;
        let mut peak = 0.0f32;
        for _ in 0..96000 * 60 {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            let x = (state >> 8) as f32 / 8388608.0 - 1.0;
            let y = f.process(x);
            peak = peak.max(y.abs());
        }
        assert!(peak.is_finite() && peak < 10.0, "runaway output: {peak}");
    }
}
