//! Analysis windows.
//!
//! Each window is generated periodically (denominator `n`, not `n-1`) because
//! the signal is assumed to continue past the frame — that is the correct form
//! for spectral analysis, and the one the published coefficients assume.
//!
//! Two normalisation constants come out of every window and both matter:
//!
//! - `s1 = Σw[i]`  — coherent gain, correct for discrete tones
//! - `s2 = Σw[i]²` — noise-power gain, correct for broadband content
//!
//! The RTA sums bins into bands, which is a noise measurement, so it uses `s2`.
//! `s1` is kept because ENBW is derived from both.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WindowKind {
    Hann,
    Hamming,
    BlackmanHarris,
    FlatTop,
    Rectangular,
}

impl WindowKind {
    pub const ALL: [WindowKind; 5] = [
        WindowKind::Hann,
        WindowKind::Hamming,
        WindowKind::BlackmanHarris,
        WindowKind::FlatTop,
        WindowKind::Rectangular,
    ];

    pub fn label(self) -> &'static str {
        match self {
            WindowKind::Hann => "Hann",
            WindowKind::Hamming => "Hamming",
            WindowKind::BlackmanHarris => "Blackman-Harris",
            WindowKind::FlatTop => "Flat-top",
            WindowKind::Rectangular => "Rectangular",
        }
    }

    /// Cosine-series coefficients. `None` is the rectangular window.
    fn coefficients(self) -> &'static [f64] {
        match self {
            WindowKind::Rectangular => &[1.0],
            WindowKind::Hann => &[0.5, 0.5],
            WindowKind::Hamming => &[0.54, 0.46],
            WindowKind::BlackmanHarris => &[0.35875, 0.48829, 0.14128, 0.01168],
            // SRS/HFT-style 5-term.
            WindowKind::FlatTop => &[
                0.215_578_95,
                0.416_631_58,
                0.277_263_158,
                0.083_578_947,
                0.006_947_368,
            ],
        }
    }

    /// One-line description, surfaced in the UI so the choice is not a mystery.
    pub fn blurb(self) -> &'static str {
        match self {
            WindowKind::Hann => "General purpose. The default for music and noise.",
            WindowKind::Hamming => "Slightly sharper than Hann, with a sidelobe shelf.",
            WindowKind::BlackmanHarris => {
                "4-term. Buys -92 dB sidelobes with a wider main lobe — use it to see a small tone next to a loud one."
            }
            WindowKind::FlatTop => {
                "Amplitude-accurate to ~0.01 dB regardless of where a tone falls between bins. Poor frequency resolution — for level calibration, not for looking at spectra."
            }
            WindowKind::Rectangular => {
                "No window at all. Best resolution, worst leakage. Only correct for signals periodic in the frame."
            }
        }
    }
}

/// A generated window plus the two sums every level calculation needs.
#[derive(Debug, Clone)]
pub struct Window {
    pub kind: WindowKind,
    pub samples: Vec<f64>,
    /// Σw[i] — coherent gain.
    pub s1: f64,
    /// Σw[i]² — noise-power gain.
    pub s2: f64,
}

impl Window {
    pub fn new(kind: WindowKind, size: usize) -> Self {
        let a = kind.coefficients();
        let n = size as f64;
        let mut samples = vec![0.0; size];

        for (i, w) in samples.iter_mut().enumerate() {
            let mut v = 0.0;
            for (k, ak) in a.iter().enumerate() {
                let sign = if k % 2 == 0 { 1.0 } else { -1.0 };
                v += sign * ak * (2.0 * std::f64::consts::PI * k as f64 * i as f64 / n).cos();
            }
            *w = v;
        }

        let s1 = samples.iter().sum();
        let s2 = samples.iter().map(|x| x * x).sum();
        Window {
            kind,
            samples,
            s1,
            s2,
        }
    }

    pub fn len(&self) -> usize {
        self.samples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// Equivalent noise bandwidth, in bins, computed from the actual
    /// coefficients rather than quoted from a table.
    pub fn enbw(&self) -> f64 {
        self.samples.len() as f64 * self.s2 / (self.s1 * self.s1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ENBW is the number every band level depends on, so it is checked against
    /// the published figures rather than against itself. Tolerances are tight:
    /// these are exact constants for the periodic form, not approximations.
    #[test]
    fn enbw_matches_published_figures() {
        let cases = [
            (WindowKind::Rectangular, 1.0),
            (WindowKind::Hann, 1.5),
            (WindowKind::Hamming, 1.3628),
            (WindowKind::BlackmanHarris, 2.0044),
            (WindowKind::FlatTop, 3.7702),
        ];
        for (kind, expected) in cases {
            let w = Window::new(kind, 8192);
            let got = w.enbw();
            assert!(
                (got - expected).abs() < 1e-3,
                "{:?}: ENBW {got} expected {expected}",
                kind
            );
        }
    }

    #[test]
    fn hann_is_periodic_not_symmetric() {
        // The periodic Hann starts at exactly 0 and never returns to 0 at the
        // end — the last sample is one step short. A symmetric window would put
        // a 0 at both ends, and would give an ENBW of 1.5 only in the limit.
        let w = Window::new(WindowKind::Hann, 8);
        assert!(w.samples[0].abs() < 1e-12);
        assert!(w.samples[7] > 0.1);
    }

    #[test]
    fn rectangular_is_all_ones() {
        let w = Window::new(WindowKind::Rectangular, 16);
        assert!(w.samples.iter().all(|&x| (x - 1.0).abs() < 1e-12));
        assert!((w.s1 - 16.0).abs() < 1e-12);
        assert!((w.s2 - 16.0).abs() < 1e-12);
    }
}
