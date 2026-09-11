//! Fractional-octave band table and the FFT-bin mapping behind it.
//!
//! Bands are base-2 and anchored on 1 kHz for every resolution:
//!
//! ```text
//! fc(k) = 1000 · 2^(k/N)          edges at fc · 2^(±1/2N)
//! ```
//!
//! IEC 61260 puts 1 kHz at a band *edge* for even N (1/6, 1/12, 1/24, 1/48) and
//! at a band centre only for odd N. We centre on 1 kHz throughout, which is what
//! every RTA a user is likely to compare against does, and which keeps the band
//! centres nested as the resolution changes — switch 1/6 → 1/48 and every 1/6
//! centre is still there. The 1/3-octave set is unaffected either way and does
//! match IEC.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Fraction {
    #[serde(rename = "1/1")]
    Octave,
    #[serde(rename = "1/3")]
    Third,
    #[serde(rename = "1/6")]
    Sixth,
    #[serde(rename = "1/12")]
    Twelfth,
    #[serde(rename = "1/24")]
    TwentyFourth,
    #[serde(rename = "1/48")]
    FortyEighth,
}

impl Fraction {
    pub const ALL: [Fraction; 6] = [
        Fraction::Octave,
        Fraction::Third,
        Fraction::Sixth,
        Fraction::Twelfth,
        Fraction::TwentyFourth,
        Fraction::FortyEighth,
    ];

    /// The N in 1/N octave.
    pub fn denominator(self) -> u32 {
        match self {
            Fraction::Octave => 1,
            Fraction::Third => 3,
            Fraction::Sixth => 6,
            Fraction::Twelfth => 12,
            Fraction::TwentyFourth => 24,
            Fraction::FortyEighth => 48,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Fraction::Octave => "1/1",
            Fraction::Third => "1/3",
            Fraction::Sixth => "1/6",
            Fraction::Twelfth => "1/12",
            Fraction::TwentyFourth => "1/24",
            Fraction::FortyEighth => "1/48",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Band {
    /// Band index k, where fc = 1000 · 2^(k/N).
    pub k: i32,
    /// Centre frequency, Hz.
    pub fc: f64,
    /// Lower edge, Hz.
    pub flo: f64,
    /// Upper edge, Hz.
    pub fhi: f64,
    /// Display label.
    pub label: String,
    /// First FFT bin inside the band. `bin_lo > bin_hi` means the band holds none.
    pub bin_lo: usize,
    /// Last FFT bin inside the band.
    pub bin_hi: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BandPlan {
    pub fraction: Fraction,
    pub fft_size: usize,
    pub sample_rate: f64,
    /// Window ENBW in bins — needed to turn one bin's power into a density.
    pub enbw: f64,
    /// Bin spacing, Hz.
    pub bin_hz: f64,
    pub bands: Vec<Band>,
    /// Lowest centre frequency whose band is at least one window-widened bin
    /// wide. Below this the display is interpolated rather than measured, and
    /// the UI shades it. Infinite if no band qualifies.
    pub resolved_above_hz: f64,
}

/// Bounds on band *centres*, not on the audible range.
///
/// They are deliberately a little outside 20 Hz – 20 kHz, and the reason is the
/// gap between nominal and exact centres. The band everyone calls "20 Hz" has an
/// exact base-2 centre of 1000·2^(-17/3) = 19.686 Hz, and the "20 kHz" band sits
/// at 1000·2^(13/3) = 20158.7 Hz. Bounding the centres at exactly 20 and 20000
/// would therefore drop both end bands from a 1/3-octave display — the two a
/// user is most likely to look for, and their absence would read as a bug.
pub const F_MIN: f64 = 19.0;
pub const F_MAX: f64 = 20_500.0;

/// ISO preferred labels for the 1/1 and 1/3-octave sets — 31.5 rather than "31".
const PREFERRED_LABELS: &[(f64, &str)] = &[
    (16.0, "16"),
    (20.0, "20"),
    (25.0, "25"),
    (31.5, "31.5"),
    (40.0, "40"),
    (50.0, "50"),
    (63.0, "63"),
    (80.0, "80"),
    (100.0, "100"),
    (125.0, "125"),
    (160.0, "160"),
    (200.0, "200"),
    (250.0, "250"),
    (315.0, "315"),
    (400.0, "400"),
    (500.0, "500"),
    (630.0, "630"),
    (800.0, "800"),
    (1000.0, "1k"),
    (1250.0, "1.25k"),
    (1600.0, "1.6k"),
    (2000.0, "2k"),
    (2500.0, "2.5k"),
    (3150.0, "3.15k"),
    (4000.0, "4k"),
    (5000.0, "5k"),
    (6300.0, "6.3k"),
    (8000.0, "8k"),
    (10000.0, "10k"),
    (12500.0, "12.5k"),
    (16000.0, "16k"),
    (20000.0, "20k"),
];

/// The nominal (labelled) frequency for a computed centre, if one is close
/// enough to be unambiguous.
fn preferred_label(fc: f64) -> Option<&'static str> {
    let mut best: Option<(&'static str, f64)> = None;
    for &(nom, label) in PREFERRED_LABELS {
        let err = (nom / fc).log2().abs();
        if best.map(|(_, e)| err < e).unwrap_or(true) {
            best = Some((label, err));
        }
    }
    // 1/3 octave is a 26% step; anything inside 3% is unambiguously that band.
    best.filter(|&(_, err)| err < 0.04).map(|(l, _)| l)
}

pub fn format_hz(f: f64) -> String {
    if f >= 10_000.0 {
        format!("{}k", (f / 100.0).round() / 10.0)
    } else if f >= 1000.0 {
        format!("{}k", (f / 10.0).round() / 100.0)
    } else if f >= 100.0 {
        format!("{}", f.round())
    } else if f >= 10.0 {
        format!("{}", (f * 10.0).round() / 10.0)
    } else {
        format!("{}", (f * 100.0).round() / 100.0)
    }
}

/// Build the band table for a resolution, transform size and sample rate.
///
/// Bands are dropped if their upper edge exceeds Nyquist — a band that is only
/// half inside the measurable spectrum would read low, and reading low without
/// saying so is worse than not drawing it.
pub fn build_band_plan(
    fraction: Fraction,
    fft_size: usize,
    sample_rate: f64,
    enbw: f64,
) -> BandPlan {
    let n = fraction.denominator() as f64;
    let bin_hz = sample_rate / fft_size as f64;
    let nyquist = sample_rate / 2.0;
    let half_step = 2f64.powf(1.0 / (2.0 * n));
    let max_bin = fft_size / 2;

    let k_start = (n * (F_MIN / 1000.0).log2()).ceil() as i32;
    let k_end = (n * (F_MAX / 1000.0).log2()).floor() as i32;

    let mut bands = Vec::new();
    for k in k_start..=k_end {
        let fc = 1000.0 * 2f64.powf(k as f64 / n);
        let flo = fc / half_step;
        let fhi = fc * half_step;
        if fhi > nyquist {
            break;
        }

        let bin_lo = (flo / bin_hz).ceil() as usize;
        let bin_hi = max_bin.min((fhi / bin_hz).floor() as usize);

        let label = if fraction.denominator() <= 3 {
            preferred_label(fc)
                .map(|s| s.to_string())
                .unwrap_or_else(|| format_hz(fc))
        } else {
            format_hz(fc)
        };

        bands.push(Band {
            k,
            fc,
            flo,
            fhi,
            label,
            bin_lo,
            bin_hi,
        });
    }

    // κ is the band's fractional width: bw = fc · κ.
    let kappa = half_step - 1.0 / half_step;
    let resolved_above_hz = (enbw * bin_hz) / kappa;

    BandPlan {
        fraction,
        fft_size,
        sample_rate,
        enbw,
        bin_hz,
        bands,
        resolved_above_hz,
    }
}

/// Integrate the bin power spectrum into band powers.
///
/// Each bin's power is taken to occupy its own cell, the `bin_hz` of spectrum
/// centred on it, and a band receives every cell it covers in proportion to
/// how much of the cell it covers. With the S2 normalisation used in
/// [`crate::spectrum`] a bin's power *is* the power in its cell — for noise,
/// PSD × `bin_hz` — so this is the plain integral of the spectrum over the
/// band: unbiased for a band many bins wide, exactly proportional to width
/// for a flat spectrum at any width, and continuous through the resolution
/// limit. No ENBW enters: that number says how *wide* a tone is smeared, not
/// how much power a cell holds, and it belongs in
/// [`BandPlan::resolved_above_hz`], which is where it is.
///
/// One path, deliberately. The previous version had two — sum the bins a
/// band contained, or, if it contained none, interpolate a density — and the
/// choice between them depended on whether a bin *centre* happened to fall
/// inside the band, which says nothing about the signal. At 1/48 octave with
/// a 2048-point transform a 1.4 Hz band at 100 Hz was credited with a whole
/// 23 Hz bin whenever a centre landed in it, and the display grew a comb of
/// bands 7–14 dB above their neighbours at every multiple of the bin spacing.
/// Whole-bin counting had a milder cousin above the limit, where a band 1.5
/// bins wide held one bin or two by luck of alignment. Fractional coverage
/// has neither, and `a_flat_spectrum_has_no_comb_at_the_bin_spacing` keeps
/// it that way.
///
/// Below [`BandPlan::resolved_above_hz`] the result is still a display
/// convenience rather than a measurement: a band narrower than the window's
/// main lobe shows the slice of that lobe it covers, not the power of a tone
/// the transform could not resolve. The UI shades that region for the reason.
pub fn integrate_bands(plan: &BandPlan, power: &[f64], out: &mut [f64]) {
    debug_assert_eq!(out.len(), plan.bands.len());
    if power.is_empty() {
        out.iter_mut().for_each(|v| *v = 0.0);
        return;
    }
    let last_bin = power.len() - 1;
    let bin_hz = plan.bin_hz;

    for (i, b) in plan.bands.iter().enumerate() {
        // Cells are [(k − ½)·bin_hz, (k + ½)·bin_hz]; these are the first and
        // last cells the band touches.
        let k_first = ((b.flo / bin_hz + 0.5).floor().max(0.0) as usize).min(last_bin);
        let k_last = ((b.fhi / bin_hz + 0.5).floor().max(0.0) as usize).min(last_bin);
        let mut acc = 0.0;
        for (k, p) in power[k_first..=k_last].iter().enumerate() {
            let k = (k_first + k) as f64;
            let cell_lo = (k - 0.5) * bin_hz;
            let cell_hi = (k + 0.5) * bin_hz;
            let overlap = b.fhi.min(cell_hi) - b.flo.max(cell_lo);
            if overlap > 0.0 {
                acc += p * (overlap / bin_hz);
            }
        }
        out[i] = acc;
    }
}

/// Power → dBFS, on the convention that a full-scale sine reads 0 dB.
///
/// A ±1.0 sine has a mean square of 0.5, so the +3.0103 dB offset is what turns
/// "RMS relative to full scale" into the peak-referenced dBFS that every meter
/// in a studio shows. The same offset is applied to broadband levels in
/// [`crate::spl`], so band levels and the meter agree.
pub const FULL_SCALE_SINE_OFFSET_DB: f64 = 3.010_299_956_639_812;

pub fn power_to_db(p: f64) -> f64 {
    10.0 * p.max(1e-30).log10() + FULL_SCALE_SINE_OFFSET_DB
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `types.ts` declares `binHz` and `resolvedAboveHz`. Emitting the Rust
    /// field names instead is invisible to TypeScript — `invoke<BandPlan>` is
    /// an unchecked cast — and lands as `undefined` at the point of use.
    #[test]
    fn the_band_plan_crosses_to_the_ui_in_camel_case() {
        let plan = build_band_plan(Fraction::Twelfth, 16384, 48000.0, 1.5);
        let json = serde_json::to_value(&plan).expect("serialises");
        for key in ["fftSize", "sampleRate", "binHz", "resolvedAboveHz"] {
            assert!(json.get(key).is_some(), "missing {key} in {json}");
        }
        for key in ["fft_size", "sample_rate", "bin_hz", "resolved_above_hz"] {
            assert!(json.get(key).is_none(), "stale {key} in {json}");
        }
        let band = &json["bands"][0];
        assert!(band.get("binLo").is_some(), "{band}");
        assert!(band.get("bin_lo").is_none(), "{band}");
    }

    #[test]
    fn third_octave_centres_are_iso_preferred() {
        let plan = build_band_plan(Fraction::Third, 16384, 48000.0, 1.5);
        let labels: Vec<&str> = plan.bands.iter().map(|b| b.label.as_str()).collect();
        for want in ["20", "31.5", "63", "125", "1k", "3.15k", "10k", "16k"] {
            assert!(labels.contains(&want), "missing {want} in {labels:?}");
        }
    }

    #[test]
    fn one_kilohertz_is_a_band_centre_at_every_resolution() {
        for fr in Fraction::ALL {
            let plan = build_band_plan(fr, 16384, 48000.0, 1.5);
            let hit = plan
                .bands
                .iter()
                .any(|b| (b.fc - 1000.0).abs() < 1e-6 && b.k == 0);
            assert!(hit, "{:?} has no band centred on 1 kHz", fr);
        }
    }

    #[test]
    fn finer_resolutions_nest_the_coarser_centres() {
        // Every 1/6-octave centre must still exist in the 1/48 set, or switching
        // resolution would move the display sideways.
        let coarse = build_band_plan(Fraction::Sixth, 32768, 48000.0, 1.5);
        let fine = build_band_plan(Fraction::FortyEighth, 32768, 48000.0, 1.5);
        for b in &coarse.bands {
            let found = fine
                .bands
                .iter()
                .any(|f| (f.fc / b.fc).log2().abs() < 1e-9);
            assert!(found, "1/48 set lost the {:.2} Hz centre", b.fc);
        }
    }

    #[test]
    fn bands_stop_below_nyquist() {
        let plan = build_band_plan(Fraction::Third, 16384, 44100.0, 1.5);
        for b in &plan.bands {
            assert!(
                b.fhi <= 22050.0,
                "band {} reaches {:.0} Hz, past Nyquist",
                b.label,
                b.fhi
            );
        }
    }

    #[test]
    fn band_edges_are_contiguous() {
        // The upper edge of band k must be the lower edge of band k+1, or the
        // spectrum has gaps and a broadband sum over bands would read low.
        let plan = build_band_plan(Fraction::Twelfth, 16384, 48000.0, 1.5);
        for pair in plan.bands.windows(2) {
            let (a, b) = (&pair[0], &pair[1]);
            assert!(
                (a.fhi - b.flo).abs() / a.fhi < 1e-12,
                "gap between {} and {}",
                a.label,
                b.label
            );
        }
    }

    /// Flat (white) power spectrum: every band's power must be proportional to
    /// its width, so a band one octave up holds twice the power. This is the
    /// property that catches an ENBW or normalisation slip.
    #[test]
    fn flat_spectrum_integrates_proportionally_to_bandwidth() {
        let plan = build_band_plan(Fraction::Third, 32768, 48000.0, 1.5);
        let bins = plan.fft_size / 2 + 1;
        let power = vec![1.0f64; bins];
        let mut out = vec![0.0f64; plan.bands.len()];
        integrate_bands(&plan, &power, &mut out);

        // Every band, whatever its width. This used to skip anything under
        // twenty bins wide because whole-bin counting could be half a bin out
        // at each edge; fractional coverage has no such quantisation, so the
        // proportionality is exact and the gate would only hide a regression.
        for (i, b) in plan.bands.iter().enumerate() {
            let expected = (b.fhi - b.flo) / plan.bin_hz;
            let err = (out[i] / expected).log10() * 10.0;
            assert!(
                err.abs() < 0.01,
                "band {} off by {err:.2} dB (got {}, expected {expected})",
                b.label,
                out[i]
            );
        }
    }

    #[test]
    fn narrow_bands_fall_back_to_interpolation() {
        // 1/48 octave at 2048 points and 48 kHz: bins are 23.4 Hz apart, so
        // every band below about 1.6 kHz is narrower than one bin.
        let plan = build_band_plan(Fraction::FortyEighth, 2048, 48000.0, 1.5);
        assert!(plan.resolved_above_hz > 1000.0);
        let empty = plan
            .bands
            .iter()
            .filter(|b| b.bin_lo > b.bin_hi)
            .count();
        assert!(empty > 0, "expected some bands to hold no bins");

        // The interpolated path must still produce finite, sane numbers.
        let bins = plan.fft_size / 2 + 1;
        let power = vec![1.0f64; bins];
        let mut out = vec![0.0f64; plan.bands.len()];
        integrate_bands(&plan, &power, &mut out);
        assert!(out.iter().all(|v| v.is_finite() && *v >= 0.0));
    }

    /// The comb seen on real input on 2026-09-11: at 1/48 octave with a
    /// 2048-point transform, every band that happened to contain a bin centre
    /// sat 7–14 dB above its neighbours, at multiples of the 23.4 Hz bin
    /// spacing, all the way up to the resolution limit. Whether a bin centre
    /// falls inside a band says nothing about the spectrum, so on a flat
    /// spectrum every band must be proportional to its width — below the
    /// resolution limit as well as above it — and no band may stand out from
    /// the ones beside it by more than the width ratio explains.
    #[test]
    fn a_flat_spectrum_has_no_comb_at_the_bin_spacing() {
        let plan = build_band_plan(Fraction::FortyEighth, 2048, 48000.0, 1.5);
        let bins = plan.fft_size / 2 + 1;
        let power = vec![1.0f64; bins];
        let mut out = vec![0.0f64; plan.bands.len()];
        integrate_bands(&plan, &power, &mut out);

        for (i, b) in plan.bands.iter().enumerate() {
            let expected = (b.fhi - b.flo) / plan.bin_hz;
            let err = (out[i] / expected).log10() * 10.0;
            assert!(
                err.abs() < 0.01,
                "band {} ({:.1} Hz) off by {err:.2} dB (got {}, expected {expected})",
                b.label,
                b.fc,
                out[i]
            );
        }
        for w in plan.bands.windows(2).zip(out.windows(2)) {
            let (bands, levels) = w;
            let step = (levels[1] / levels[0]).log10() * 10.0;
            let width_ratio = ((bands[1].fhi - bands[1].flo) / (bands[0].fhi - bands[0].flo)).log10() * 10.0;
            assert!(
                (step - width_ratio).abs() < 0.01,
                "{} -> {} steps {step:.2} dB where the widths explain {width_ratio:.2} dB",
                bands[0].label,
                bands[1].label
            );
        }
    }

    /// Integer bin counting has a second artefact just above the resolution
    /// limit: a band 1.5 bins wide holds one bin or two depending on where its
    /// edges fall, a 3 dB sawtooth on a flat spectrum. Fractional coverage
    /// removes it, at every width.
    #[test]
    fn bands_a_few_bins_wide_do_not_alternate() {
        let plan = build_band_plan(Fraction::Twelfth, 4096, 48000.0, 1.5);
        let bins = plan.fft_size / 2 + 1;
        let power = vec![1.0f64; bins];
        let mut out = vec![0.0f64; plan.bands.len()];
        integrate_bands(&plan, &power, &mut out);
        for (i, b) in plan.bands.iter().enumerate() {
            let width_bins = (b.fhi - b.flo) / plan.bin_hz;
            if !(1.0..=4.0).contains(&width_bins) {
                continue;
            }
            let err = (out[i] / width_bins).log10() * 10.0;
            assert!(err.abs() < 0.01, "band {} ({width_bins:.2} bins) off by {err:.2} dB", b.label);
        }
    }

    /// A tone's power must not depend on where the band edges fall relative
    /// to the bins that carry it: summed across the bands that cover its main
    /// lobe it is the tone's mean square, whether one band holds the lobe or
    /// three share it. This is what the S2 normalisation promises, and
    /// fractional coverage must keep the promise — every bin's power is
    /// handed out exactly once.
    #[test]
    fn bin_power_is_conserved_across_band_edges() {
        let plan = build_band_plan(Fraction::Third, 8192, 48000.0, 1.5);
        let bins = plan.fft_size / 2 + 1;
        let mut power = vec![0.0f64; bins];
        // A Hann main lobe centred between two bins, straddling a band edge.
        let edge_bin = (plan.bands[10].fhi / plan.bin_hz).round() as usize;
        power[edge_bin - 1] = 0.25;
        power[edge_bin] = 0.5;
        power[edge_bin + 1] = 0.25;
        let mut out = vec![0.0f64; plan.bands.len()];
        integrate_bands(&plan, &power, &mut out);
        let total: f64 = out.iter().sum();
        assert!((total - 1.0).abs() < 1e-9, "lobe power {total} != 1.0");
    }
}
