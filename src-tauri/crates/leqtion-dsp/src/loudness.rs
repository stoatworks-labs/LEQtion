//! Programme loudness: ITU-R BS.1770 as EBU R 128 meters it.
//!
//! Five numbers, all from one K-weighted signal path:
//!
//! | Reading | Window | Gated | Where it is defined |
//! |---|---|---|---|
//! | Momentary (M) | 400 ms sliding | no | EBU Tech 3341 §2.2 (1) |
//! | Short-term (S) | 3 s sliding | no | EBU Tech 3341 §2.2 (2) |
//! | Integrated (I) | since reset | −70 LUFS absolute, −10 LU relative | ITU-R BS.1770-5 Annex 1, eqs. (3)–(7) |
//! | Loudness range (LRA) | since reset | −70 LUFS absolute, −20 LU relative | EBU Tech 3342 §3.1 |
//! | True peak | since reset | — | ITU-R BS.1770-5 Annex 2 |
//!
//! ## Loudness is not a sound level, and never sees the calibration
//!
//! LUFS is "loudness, K-weighted, relative to full scale" (BS.1770-5 Annex 1).
//! It describes a programme signal, not a sound field, so no calibration offset
//! ever reaches it — a calibrated microphone input still reads LUFS here, and
//! the number means what it means on any other R 128 meter.
//!
//! It also does not share the rest of the app's dBFS convention. The levels
//! elsewhere in this crate put a full-scale sine at 0 dBFS; BS.1770 puts a
//! full-scale 997 Hz sine on one channel at **−3.01 LUFS**, because equation (2)
//! is a plain mean square and the −0.691 constant only cancels the K-filter's
//! gain. Two meters three decibels apart on the same tone are both right.
//! `a_full_scale_sine_on_one_channel_reads_minus_three_lufs` pins it.
//!
//! ## The K-weighting filter
//!
//! BS.1770 tabulates both biquads at 48 kHz only and asks for "the same
//! frequency response" at other rates. The coefficients here are derived from
//! the analogue parameters of each stage — a shelf at 1681.97 Hz and an RLB
//! high-pass at 38.135 Hz — bilinear-transformed at the actual rate, which is
//! the derivation libebur128 (MIT, Jan Kokemüller) uses. At 48 kHz it
//! reproduces the standard's table (`the_standard_48k_coefficients_are_reproduced`),
//! and at 44.1 to 192 kHz it stays within 0.01 dB of the 48 kHz response from
//! 20 Hz to 20 kHz, which needs one correction libebur128 does not make — see
//! the high-pass in `k_weighting`.
//!
//! ## Why the windows slide in 10 ms steps, not 100 ms
//!
//! BS.1770's gating blocks are 400 ms long and start every 100 ms, so 100 ms is
//! the obvious step. It is not enough for the maxima. Tech 3341 cases 13 and 14
//! put a 400 ms tone at offsets of 20 ms and expect Max M within 0.1 LU of the
//! tone; on a 100 ms grid the best window can miss 50 ms of it, which reads
//! 0.58 LU low. So the window is built from 10 ms ticks: momentary and
//! short-term (and their maxima) move every tick, and every tenth tick is a
//! gating block for the integrated loudness — exactly BS.1770's 75 % overlap —
//! and a short-term value for the loudness range, comfortably above Tech 3342's
//! 10 Hz minimum. At a rate with no whole number of samples in 10 ms (22.05 kHz,
//! 11.025 kHz) a tick is the nearest whole number, which changes the windows by
//! under 0.25 %.
//!
//! ## Integrated loudness and loudness range in bounded memory
//!
//! Both are statistics over every block since the reset, and a relative gate
//! means the whole set has to be re-examined whenever it changes: the threshold
//! is itself a loudness of the set (Tech 3341 §2.3, note). Keeping every block
//! grows without limit over a long measurement, so each measure keeps a
//! histogram instead: 0.01 LU bins from −70 to +30 LUFS, each holding a count
//! and the **sum of the blocks' energies**. Two consequences:
//!
//! - Every mean is still an energy mean of the actual blocks, never of the bin
//!   centres, so a set of blocks that all pass the gate gives an exact answer.
//!   Bin edges start at −70, so the absolute gate is exact too.
//! - A bin passes or fails the relative gate as a whole, judged by the loudness
//!   of its own mean energy. The only blocks that can be misclassified are
//!   those within 0.01 LU of the relative threshold, and if a fraction `p` of
//!   the included blocks is in that position the integrated loudness is out by
//!   at most `−10·log10(1 − p)` LU — 0.04 LU if one block in a hundred sits on
//!   the gate. On a steady tone it is zero: identical blocks share a bin.
//!   Loudness range takes its percentiles from bin means, so it is within
//!   0.02 LU of the exact sort. `the_histogram_agrees_with_an_exact_gate`
//!   measures both against the unbinned algorithm.
//!
//! ## True peak
//!
//! Annex 2's 48-tap, 4-phase interpolating filter, applied at every sample
//! rate. The Annex notes that 2× would do at 96 kHz; 4× there costs a little
//! more and reads no worse. The original samples are counted as well as the
//! interpolated ones — the reconstructed waveform passes through them, so a
//! true peak below the sample peak would be an artefact of the filter's ripple.
//! The Annex's worst-case under-read for 4× at a quarter of the sample rate is
//! 0.69 dB; Tech 3341 cases 15–23 allow +0.2/−0.4 dB.
//!
//! ## What a reset does
//!
//! It restarts the measurement — integrated, range, both maxima, the true peak
//! and the elapsed time — and leaves the filters and the momentary and
//! short-term windows running, for the reason given at `Path::reset_statistics`
//! in [`crate::engine`]: those describe the signal now, not the measurement.
//! A tick in progress at the moment of the reset holds audio from before it, so
//! the measurement starts at the next tick boundary, at most 10 ms later.
//!
//! Loudness range is defined once one short-term window has passed its gates,
//! three seconds after a reset, but Tech 3341 §2.4 requires a meter to show it
//! as not yet stable for the first 60 s. That is the display's job; the reading
//! carries `elapsed_seconds` so it can.

use serde::Serialize;

use crate::spl::{amplitude_to_dbfs, SILENCE_DBFS};
use crate::weighting::Biquad;

/// BS.1770-5 eq. (2). Cancels the K-weighting's gain at 997 Hz (Note 1).
const LOUDNESS_OFFSET: f64 = -0.691;

/// Ticks per second. See the module docs for why the step is not 100 ms.
const TICKS_PER_SECOND: f64 = 100.0;
/// 400 ms. Tech 3341 §2.2 (1); BS.1770-5 eq. (3), Tg.
const MOMENTARY_TICKS: usize = 40;
/// 3 s. Tech 3341 §2.2 (2); Tech 3342 §3.1.
const SHORT_TERM_TICKS: usize = 300;
/// 100 ms between gating blocks: 75 % overlap of 400 ms, BS.1770-5 eq. (3).
/// The same step samples short-term loudness at 10 Hz for the loudness range,
/// the minimum Tech 3342 §3.1 allows.
const STEP_TICKS: i64 = 10;

/// BS.1770-5 eq. (6), Γa. Also Tech 3342 §3.1's absolute threshold.
const ABSOLUTE_GATE: f64 = -70.0;
/// BS.1770-5 eq. (6): Γr is 10 LU below the absolute-gated loudness.
const RELATIVE_GATE: f64 = -10.0;
/// Tech 3342 §3.1: the loudness range gates 20 LU below, not 10.
const RANGE_RELATIVE_GATE: f64 = -20.0;
/// Tech 3342 §3.1: the 10th and 95th percentiles.
const RANGE_LOW_PERCENTILE: f64 = 0.10;
const RANGE_HIGH_PERCENTILE: f64 = 0.95;

/// Histogram bins, 0.01 LU from the absolute gate up. Anything louder than
/// +30 LUFS lands in the top bin; its energy is still summed exactly.
const HISTOGRAM_FLOOR: f64 = ABSOLUTE_GATE;
const HISTOGRAM_STEP: f64 = 0.01;
const HISTOGRAM_BINS: usize = 10_000;

/// K-weighting stage 1, the shelf that models the head (BS.1770-5 Annex 1,
/// Table 1), by its analogue parameters. These and the stage 2 values below
/// are libebur128's; at 48 kHz they reproduce Tables 1 and 2.
const SHELF_HZ: f64 = 1681.974450955533;
const SHELF_GAIN_DB: f64 = 3.999843853973347;
const SHELF_Q: f64 = 0.7071752369554196;
/// The shelf's mid-point gain, as a power of the high-frequency gain.
const SHELF_MID_EXPONENT: f64 = 0.4996667741545416;
/// K-weighting stage 2, the RLB high-pass (Table 2).
const HIGH_PASS_HZ: f64 = 38.13547087602444;
const HIGH_PASS_Q: f64 = 0.5003270373238773;

/// Taps per phase of the true-peak interpolator.
const TRUE_PEAK_TAPS: usize = 12;

/// The order-48, 4-phase FIR interpolating filter of BS.1770-5 Annex 2, one
/// row per phase, `h[k]` applied to the sample `k` steps back.
const TRUE_PEAK_PHASES: [[f64; TRUE_PEAK_TAPS]; 4] = [
    [
        0.0017089843750,
        0.0109863281250,
        -0.0196533203125,
        0.0332031250000,
        -0.0594482421875,
        0.1373291015625,
        0.9721679687500,
        -0.1022949218750,
        0.0476074218750,
        -0.0266113281250,
        0.0148925781250,
        -0.0083007812500,
    ],
    [
        -0.0291748046875,
        0.0292968750000,
        -0.0517578125000,
        0.0891113281250,
        -0.1665039062500,
        0.4650878906250,
        0.7797851562500,
        -0.2003173828125,
        0.1015625000000,
        -0.0582275390625,
        0.0330810546875,
        -0.0189208984375,
    ],
    [
        -0.0189208984375,
        0.0330810546875,
        -0.0582275390625,
        0.1015625000000,
        -0.2003173828125,
        0.7797851562500,
        0.4650878906250,
        -0.1665039062500,
        0.0891113281250,
        -0.0517578125000,
        0.0292968750000,
        -0.0291748046875,
    ],
    [
        -0.0083007812500,
        0.0148925781250,
        -0.0266113281250,
        0.0476074218750,
        -0.1022949218750,
        0.9721679687500,
        0.1373291015625,
        -0.0594482421875,
        0.0332031250000,
        -0.0196533203125,
        0.0109863281250,
        0.0017089843750,
    ],
];

/// The largest gain any phase of the interpolator can apply to a window of
/// samples, `max_p Σ|h_p[k]|` (phases 1 and 2 above, exactly). If every sample
/// in the window is below `peak / TRUE_PEAK_GAIN_BOUND`, no phase can exceed
/// `peak`, so the interpolation is skipped — which on programme material, once
/// a peak has been seen, is most samples more than 6 dB below it.
const TRUE_PEAK_GAIN_BOUND: f64 = 2.0228271484375;

/// Weighted mean square → LUFS. BS.1770-5 eq. (2), with this crate's floor.
fn loudness(energy: f64) -> f64 {
    if energy <= 0.0 {
        return SILENCE_DBFS;
    }
    (LOUDNESS_OFFSET + 10.0 * energy.log10()).max(SILENCE_DBFS)
}

/// The two K-weighting biquads at `sample_rate`, bilinear-transformed from
/// their analogue parameters.
fn k_weighting(sample_rate: f64) -> [Biquad; 2] {
    use std::f64::consts::PI;

    let k = (PI * SHELF_HZ / sample_rate).tan();
    let high = 10f64.powf(SHELF_GAIN_DB / 20.0);
    let mid = high.powf(SHELF_MID_EXPONENT);
    let a0 = 1.0 + k / SHELF_Q + k * k;
    let mut shelf = Biquad::default();
    shelf.b0 = (high + mid * k / SHELF_Q + k * k) / a0;
    shelf.b1 = 2.0 * (k * k - high) / a0;
    shelf.b2 = (high - mid * k / SHELF_Q + k * k) / a0;
    shelf.a1 = 2.0 * (k * k - 1.0) / a0;
    shelf.a2 = (1.0 - k / SHELF_Q + k * k) / a0;

    let high_pass_a0 = |rate: f64| {
        let k = (PI * HIGH_PASS_HZ / rate).tan();
        1.0 + k / HIGH_PASS_Q + k * k
    };
    let k = (PI * HIGH_PASS_HZ / sample_rate).tan();
    let a0 = high_pass_a0(sample_rate);
    // Table 2's numerator is 1, −2, 1 and is not divided by a0, so the 48 kHz
    // filter passes everything above its corner a0(48 kHz) ≈ 1.004995 times
    // up: +0.043 dB, part of the response the standard specifies and part of
    // what the −0.691 calibrates out. A transform at another rate has another
    // a0, so the numerator is scaled to keep the 48 kHz gain. Left at 1, −2, 1,
    // as libebur128 leaves it, every reading falls 0.02 LU at 96 kHz and
    // 0.03 LU at 192 kHz, flat across the band — measured, and pinned by
    // `the_k_filter_has_the_same_response_at_every_rate`.
    let gain = high_pass_a0(48000.0) / a0;
    let mut high_pass = Biquad::default();
    high_pass.b0 = gain;
    high_pass.b1 = -2.0 * gain;
    high_pass.b2 = gain;
    high_pass.a1 = 2.0 * (k * k - 1.0) / a0;
    high_pass.a2 = (1.0 - k / HIGH_PASS_Q + k * k) / a0;

    [shelf, high_pass]
}

/// The readout. Everything is LUFS except `range` (LU) and `true_peak` (dBTP).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoudnessReading {
    /// None until 400 ms of audio has been measured.
    pub momentary: Option<f64>,
    /// None until 3 s.
    pub short_term: Option<f64>,
    /// None until a gating block since the reset has passed both gates.
    pub integrated: Option<f64>,
    /// Loudness range, LU. None until a short-term window since the reset has
    /// passed both gates. Not to be shown as settled before 60 s; see the
    /// module docs.
    pub range: Option<f64>,
    /// Highest momentary loudness since the reset (Tech 3341 §2.1).
    pub max_momentary: Option<f64>,
    pub max_short_term: Option<f64>,
    /// Highest true peak since the reset across every measured channel, dBTP.
    pub true_peak: Option<f64>,
    /// Seconds of audio since the reset.
    pub elapsed_seconds: f64,
}

/// Block loudnesses by 0.01 LU bin: a count and an energy sum per bin.
#[derive(Debug, Clone)]
struct Histogram {
    count: Vec<u64>,
    energy: Vec<f64>,
    total_count: u64,
    total_energy: f64,
    /// Lowest and highest occupied bins, so a scan covers only those.
    lo: usize,
    hi: usize,
}

impl Histogram {
    fn new() -> Self {
        Histogram {
            count: vec![0; HISTOGRAM_BINS],
            energy: vec![0.0; HISTOGRAM_BINS],
            total_count: 0,
            total_energy: 0.0,
            lo: HISTOGRAM_BINS,
            hi: 0,
        }
    }

    /// Record one block. The caller has already applied the absolute gate.
    fn add(&mut self, lufs: f64, energy: f64) {
        let bin = ((lufs - HISTOGRAM_FLOOR) / HISTOGRAM_STEP).floor().max(0.0) as usize;
        let bin = bin.min(HISTOGRAM_BINS - 1);
        self.count[bin] += 1;
        self.energy[bin] += energy;
        self.total_count += 1;
        self.total_energy += energy;
        self.lo = self.lo.min(bin);
        self.hi = self.hi.max(bin);
    }

    fn clear(&mut self) {
        if self.total_count > 0 {
            self.count[self.lo..=self.hi].fill(0);
            self.energy[self.lo..=self.hi].fill(0.0);
        }
        self.total_count = 0;
        self.total_energy = 0.0;
        self.lo = HISTOGRAM_BINS;
        self.hi = 0;
    }

    /// Occupied bins in ascending order, as `(count, energy sum, loudness of
    /// the bin's mean energy)`.
    fn bins(&self) -> impl Iterator<Item = (u64, f64, f64)> + '_ {
        let range = if self.total_count == 0 {
            0..0
        } else {
            self.lo..self.hi + 1
        };
        range.filter_map(|i| {
            let c = self.count[i];
            (c > 0).then(|| (c, self.energy[i], loudness(self.energy[i] / c as f64)))
        })
    }

    /// Loudness of the energy mean of every block recorded: the absolute-gated
    /// loudness both relative gates are measured from.
    fn mean_loudness(&self) -> Option<f64> {
        (self.total_count > 0).then(|| loudness(self.total_energy / self.total_count as f64))
    }
}

/// Integrated loudness from gating blocks that have passed the absolute gate.
/// BS.1770-5 eq. (7): blocks *above* Γr.
fn integrated_from(blocks: &Histogram) -> Option<f64> {
    let gate = blocks.mean_loudness()? + RELATIVE_GATE;
    let (mut n, mut e) = (0u64, 0.0f64);
    for (c, sum, lufs) in blocks.bins() {
        if lufs > gate {
            n += c;
            e += sum;
        }
    }
    (n > 0).then(|| loudness(e / n as f64))
}

/// Loudness range from short-term values that have passed the absolute gate.
/// Tech 3342 §5: values *at or above* the relative gate, percentiles by
/// `round((n − 1)·p + 1)` into the sorted list.
fn range_from(short_terms: &Histogram) -> Option<f64> {
    let gate = short_terms.mean_loudness()? + RANGE_RELATIVE_GATE;
    let n: u64 = short_terms
        .bins()
        .filter(|&(_, _, lufs)| lufs >= gate)
        .map(|(c, _, _)| c)
        .sum();
    if n == 0 {
        return None;
    }
    // The 1-based rank of a percentile, as the reference MATLAB computes it.
    let rank = |p: f64| ((n - 1) as f64 * p + 1.0).round() as u64;
    let at = |rank: u64| {
        let mut seen = 0u64;
        short_terms
            .bins()
            .filter(|&(_, _, lufs)| lufs >= gate)
            .find(|&(c, _, _)| {
                seen += c;
                seen >= rank
            })
            .map(|(_, _, lufs)| lufs)
    };
    Some(at(rank(RANGE_HIGH_PERCENTILE))? - at(rank(RANGE_LOW_PERCENTILE))?)
}

/// One channel's true-peak interpolator.
#[derive(Debug, Clone)]
struct TruePeak {
    /// The last [`TRUE_PEAK_TAPS`] samples, stored twice over so the window is
    /// always one contiguous slice: `history[pos + 1..=pos + TAPS]`, oldest
    /// first.
    history: [f64; 2 * TRUE_PEAK_TAPS],
    pos: usize,
    /// Samples taken since the start or the last reset, up to a full window.
    /// Until the window is full it holds samples from before the measurement
    /// (or zeros from before the stream), and an interpolation across them
    /// describes a waveform that was never in this measurement.
    fill: usize,
}

impl TruePeak {
    fn new() -> Self {
        TruePeak {
            history: [0.0; 2 * TRUE_PEAK_TAPS],
            pos: 0,
            fill: 0,
        }
    }

    fn restart(&mut self) {
        self.fill = 0;
    }

    /// Take one sample and return the highest magnitude of the reconstructed
    /// waveform around it, or `held` if nothing near it can exceed `held`.
    #[inline]
    fn push(&mut self, x: f64, held: f64) -> f64 {
        self.pos = (self.pos + 1) % TRUE_PEAK_TAPS;
        self.history[self.pos] = x;
        self.history[self.pos + TRUE_PEAK_TAPS] = x;
        let mut peak = held.max(x.abs());
        self.fill = (self.fill + 1).min(TRUE_PEAK_TAPS);
        if self.fill < TRUE_PEAK_TAPS {
            return peak;
        }

        // Newest last, so the sample k steps back is `window[TAPS - 1 - k]`.
        let window: &[f64; TRUE_PEAK_TAPS] = self.history[self.pos + 1..=self.pos + TRUE_PEAK_TAPS]
            .try_into()
            .expect("the window is TRUE_PEAK_TAPS long");
        let mut largest = 0.0f64;
        for s in window {
            largest = largest.max(s.abs());
        }
        if largest * TRUE_PEAK_GAIN_BOUND <= held {
            return held;
        }

        // Plain indexed loops: this runs four times per sample per channel, and
        // in an unoptimised build an iterator chain here costs the test suite
        // more than everything else in it.
        for phase in &TRUE_PEAK_PHASES {
            let mut y = 0.0;
            for k in 0..TRUE_PEAK_TAPS {
                y += phase[k] * window[TRUE_PEAK_TAPS - 1 - k];
            }
            peak = peak.max(y.abs());
        }
        peak
    }
}

#[derive(Debug, Clone)]
struct Channel {
    k: [Biquad; 2],
    peak: TruePeak,
    weight: f64,
}

/// An R 128 loudness meter over one programme of up to any number of channels.
#[derive(Debug, Clone)]
pub struct LoudnessMeter {
    sample_rate: f64,
    weights: Vec<f64>,
    channels: Vec<Channel>,

    tick_len: usize,
    tick_pos: usize,
    /// Σ Gᵢ·yᵢ² over the tick so far.
    tick_acc: f64,
    /// Weighted mean square of each of the last [`SHORT_TERM_TICKS`] ticks.
    ticks: [f64; SHORT_TERM_TICKS],
    /// Where the next tick is written.
    tick_head: usize,
    /// Ticks completed since the meter was built. Decides when M and S exist.
    ticks_total: u64,
    /// Whole ticks completed since the measurement started. −1 while the tick
    /// that straddles a reset finishes. Decides gating blocks and maxima.
    since_reset: i64,

    momentary: Option<f64>,
    short_term: Option<f64>,
    max_momentary: Option<f64>,
    max_short_term: Option<f64>,
    blocks: Histogram,
    short_terms: Histogram,
    /// Recomputed when a gating block or short-term value arrives — ten times
    /// a second — rather than on every [`LoudnessMeter::reading`].
    integrated: Option<f64>,
    range: Option<f64>,
    true_peak: f64,
    elapsed_frames: u64,
}

impl LoudnessMeter {
    /// One weight per measured channel: BS.1770-5 Table 3 gives 1.0 for left,
    /// right and centre and 1.41 for the surrounds. The LFE channel is not
    /// measured at all (Annex 1), so it is simply not one of the channels.
    pub fn new(sample_rate: f64, weights: &[f64]) -> Self {
        let channels = weights
            .iter()
            .map(|&weight| Channel {
                k: k_weighting(sample_rate),
                peak: TruePeak::new(),
                weight,
            })
            .collect();
        LoudnessMeter {
            sample_rate,
            weights: weights.to_vec(),
            channels,
            tick_len: ((sample_rate / TICKS_PER_SECOND).round() as usize).max(1),
            tick_pos: 0,
            tick_acc: 0.0,
            ticks: [0.0; SHORT_TERM_TICKS],
            tick_head: 0,
            ticks_total: 0,
            since_reset: 0,
            momentary: None,
            short_term: None,
            max_momentary: None,
            max_short_term: None,
            blocks: Histogram::new(),
            short_terms: Histogram::new(),
            integrated: None,
            range: None,
            true_peak: 0.0,
            elapsed_frames: 0,
        }
    }

    /// Rebuild for a new rate or channel set, which restarts everything. A
    /// no-op when nothing changed, so it is safe to call on every reconfigure.
    pub fn reconfigure(&mut self, sample_rate: f64, weights: &[f64]) {
        let rate_changed = (sample_rate - self.sample_rate).abs() > f64::EPSILON;
        if rate_changed || weights != self.weights.as_slice() {
            *self = LoudnessMeter::new(sample_rate, weights);
        }
    }

    /// Feed interleaved input. `picks[i]` is the interleaved index of measured
    /// channel `i`; the caller clamps them to the stream's channel count.
    pub fn push_interleaved(&mut self, samples: &[f32], channels: usize, picks: &[usize]) {
        if channels == 0 || self.channels.is_empty() {
            return;
        }
        for frame in samples.chunks_exact(channels) {
            let mut sum = 0.0f64;
            for (ch, &pick) in self.channels.iter_mut().zip(picks) {
                let x = frame[pick.min(channels - 1)] as f64;
                let [shelf, high_pass] = &mut ch.k;
                let y = high_pass.process(shelf.process(x));
                sum += ch.weight * y * y;
                self.true_peak = ch.peak.push(x, self.true_peak);
            }
            self.tick_acc += sum;
            self.tick_pos += 1;
            self.elapsed_frames += 1;
            if self.tick_pos == self.tick_len {
                self.complete_tick();
            }
        }
    }

    fn complete_tick(&mut self) {
        self.ticks[self.tick_head] = self.tick_acc / self.tick_len as f64;
        self.tick_head = (self.tick_head + 1) % SHORT_TERM_TICKS;
        self.tick_acc = 0.0;
        self.tick_pos = 0;
        self.ticks_total += 1;
        self.since_reset += 1;

        if self.ticks_total >= MOMENTARY_TICKS as u64 {
            self.momentary = Some(self.window_energy(MOMENTARY_TICKS));
        }
        if self.ticks_total >= SHORT_TERM_TICKS as u64 {
            self.short_term = Some(self.window_energy(SHORT_TERM_TICKS));
        }

        let since = self.since_reset;
        if let (Some(m), true) = (self.momentary, since >= MOMENTARY_TICKS as i64) {
            self.max_momentary = Some(self.max_momentary.map_or(m, |x| x.max(m)));
            if (since - MOMENTARY_TICKS as i64) % STEP_TICKS == 0 {
                let lufs = loudness(m);
                // Strictly above: BS.1770-5 eq. (6), Jg = { j : lj > Γa }.
                if lufs > ABSOLUTE_GATE {
                    self.blocks.add(lufs, m);
                    self.integrated = integrated_from(&self.blocks);
                }
            }
        }
        if let (Some(s), true) = (self.short_term, since >= SHORT_TERM_TICKS as i64) {
            self.max_short_term = Some(self.max_short_term.map_or(s, |x| x.max(s)));
            if (since - SHORT_TERM_TICKS as i64) % STEP_TICKS == 0 {
                let lufs = loudness(s);
                // At or above: Tech 3342 §5, `ShortTermLoudness >= ABS_THRES`.
                if lufs >= ABSOLUTE_GATE {
                    self.short_terms.add(lufs, s);
                    self.range = range_from(&self.short_terms);
                }
            }
        }
    }

    /// Mean of the most recent `n` ticks. Ticks are all the same length, so the
    /// mean of their mean squares is the mean square of the whole window.
    /// Summed afresh each time rather than kept as a running total, which over
    /// hours would drift.
    fn window_energy(&self, n: usize) -> f64 {
        let mut sum = 0.0;
        for k in 1..=n {
            sum += self.ticks[(self.tick_head + SHORT_TERM_TICKS - k) % SHORT_TERM_TICKS];
        }
        sum / n as f64
    }

    pub fn reading(&self) -> LoudnessReading {
        LoudnessReading {
            momentary: self.momentary.map(loudness),
            short_term: self.short_term.map(loudness),
            integrated: self.integrated,
            range: self.range,
            max_momentary: self.max_momentary.map(loudness),
            max_short_term: self.max_short_term.map(loudness),
            true_peak: (self.elapsed_frames > 0).then(|| amplitude_to_dbfs(self.true_peak)),
            elapsed_seconds: self.elapsed_frames as f64 / self.sample_rate,
        }
    }

    /// Start a new measurement. See the module docs for what survives.
    pub fn reset(&mut self) {
        self.since_reset = if self.tick_pos == 0 { 0 } else { -1 };
        self.max_momentary = None;
        self.max_short_term = None;
        self.blocks.clear();
        self.short_terms.clear();
        self.integrated = None;
        self.range = None;
        self.true_peak = 0.0;
        for ch in &mut self.channels {
            ch.peak.restart();
        }
        self.elapsed_frames = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    const RATE: f64 = 48000.0;
    const STEREO: [f64; 2] = [1.0, 1.0];

    /// Per-channel peak level in dBFS, as Tech 3341 and 3342 state their tones.
    fn amplitude(dbfs: f64) -> f64 {
        10f64.powf(dbfs / 20.0)
    }

    /// Segments of a 1 kHz tone (`Some(dBFS)`) or silence (`None`), the same
    /// on every channel and continuous in phase across segments — the shape
    /// of every tone sequence in Tech 3341 and 3342.
    fn tones(rate: f64, channels: usize, segments: &[(Option<f64>, f64)]) -> Vec<f32> {
        let mut out = Vec::new();
        let mut n = 0u64;
        for &(level, seconds) in segments {
            let frames = (seconds * rate).round() as u64;
            for _ in 0..frames {
                let s = match level {
                    Some(db) => amplitude(db) * (2.0 * PI * 1000.0 * n as f64 / rate).sin(),
                    None => 0.0,
                };
                for _ in 0..channels {
                    out.push(s as f32);
                }
                n += 1;
            }
        }
        out
    }

    /// Push in ragged chunks, so ticks straddle calls as they do in the app.
    fn feed(meter: &mut LoudnessMeter, buf: &[f32], channels: usize) {
        let picks: Vec<usize> = (0..channels).collect();
        for chunk in buf.chunks(1031 * channels) {
            meter.push_interleaved(chunk, channels, &picks);
        }
    }

    fn measure(rate: f64, segments: &[(Option<f64>, f64)]) -> LoudnessReading {
        let mut m = LoudnessMeter::new(rate, &STEREO);
        feed(&mut m, &tones(rate, 2, segments), 2);
        m.reading()
    }

    fn within(got: Option<f64>, want: f64, tol: f64, what: &str) {
        let got = got.unwrap_or_else(|| panic!("{what}: no reading"));
        assert!(
            (got - want).abs() <= tol,
            "{what}: {got:.3}, expected {want} ± {tol}"
        );
    }

    /// |H(f)| of a biquad cascade, in dB.
    fn response_db(stages: &[Biquad], hz: f64, rate: f64) -> f64 {
        let w = 2.0 * PI * hz / rate;
        let (c1, s1, c2, s2) = (w.cos(), w.sin(), (2.0 * w).cos(), (2.0 * w).sin());
        let mut mag = 1.0;
        for q in stages {
            let (nr, ni) = (q.b0 + q.b1 * c1 + q.b2 * c2, -(q.b1 * s1 + q.b2 * s2));
            let (dr, di) = (1.0 + q.a1 * c1 + q.a2 * c2, -(q.a1 * s1 + q.a2 * s2));
            mag *= ((nr * nr + ni * ni) / (dr * dr + di * di)).sqrt();
        }
        20.0 * mag.log10()
    }

    // --- The filter --------------------------------------------------------

    /// BS.1770-5 Annex 1, Tables 1 and 2, to every digit printed: the derived
    /// coefficients agree to within 1e-15 (measured 2026-10-25).
    #[test]
    fn the_standard_48k_coefficients_are_reproduced() {
        let [shelf, hp] = k_weighting(48000.0);
        let table_1 = [
            (shelf.b0, 1.53512485958697),
            (shelf.b1, -2.69169618940638),
            (shelf.b2, 1.19839281085285),
            (shelf.a1, -1.69065929318241),
            (shelf.a2, 0.73248077421585),
        ];
        for (got, want) in table_1 {
            assert!((got - want).abs() < 1e-12, "shelf {got} vs Table 1 {want}");
        }
        let table_2 = [
            (hp.b0, 1.0),
            (hp.b1, -2.0),
            (hp.b2, 1.0),
            (hp.a1, -1.99004745483398),
            (hp.a2, 0.99007225036621),
        ];
        for (got, want) in table_2 {
            assert!(
                (got - want).abs() < 1e-12,
                "high-pass {got} vs Table 2 {want}"
            );
        }
    }

    /// BS.1770 Note 1: the −0.691 constant exists to cancel this.
    #[test]
    fn the_k_filter_gains_0_691_db_at_997_hz() {
        let gain = response_db(&k_weighting(48000.0), 997.0, 48000.0);
        assert!((gain - 0.691).abs() < 0.001, "{gain}");
    }

    /// "The same frequency response that the specified filter provides at
    /// 48 kHz" — derived coefficients at other rates must keep it, at least
    /// where the bilinear transform's warping has not yet set in.
    #[test]
    fn the_k_filter_has_the_same_response_at_every_rate() {
        let reference = k_weighting(48000.0);
        for rate in [44100.0, 88200.0, 96000.0, 192000.0] {
            let stages = k_weighting(rate);
            for hz in [
                20.0, 38.0, 100.0, 997.0, 1682.0, 3000.0, 5000.0, 10000.0, 20000.0,
            ] {
                let a = response_db(&reference, hz, 48000.0);
                let b = response_db(&stages, hz, rate);
                assert!(
                    (a - b).abs() < 0.01,
                    "{rate} Hz rate at {hz} Hz: {b:.4} dB vs {a:.4} dB at 48 kHz"
                );
            }
        }
    }

    // --- Momentary, short-term, integrated: Tech 3341 Table 1 ---------------

    #[test]
    fn tech_3341_case_1_stereo_tone_at_minus_23() {
        let r = measure(RATE, &[(Some(-23.0), 20.0)]);
        within(r.momentary, -23.0, 0.1, "M");
        within(r.short_term, -23.0, 0.1, "S");
        within(r.integrated, -23.0, 0.1, "I");
    }

    #[test]
    fn tech_3341_case_2_stereo_tone_at_minus_33() {
        let r = measure(RATE, &[(Some(-33.0), 20.0)]);
        within(r.momentary, -33.0, 0.1, "M");
        within(r.short_term, -33.0, 0.1, "S");
        within(r.integrated, -33.0, 0.1, "I");
    }

    /// The relative gate: the −36 dBFS tones are more than 10 LU below the
    /// absolute-gated loudness, so they are excluded and I is the −23 tone's.
    #[test]
    fn tech_3341_case_3_the_relative_gate_excludes_quiet_passages() {
        let r = measure(
            RATE,
            &[
                (Some(-36.0), 10.0),
                (Some(-23.0), 60.0),
                (Some(-36.0), 10.0),
            ],
        );
        within(r.integrated, -23.0, 0.1, "I");
    }

    /// The absolute gate: −72 dBFS is below −70 LUFS and must not drag the
    /// relative threshold down far enough to admit the −36 tones.
    #[test]
    fn tech_3341_case_4_the_absolute_gate_excludes_near_silence() {
        let r = measure(
            RATE,
            &[
                (Some(-72.0), 10.0),
                (Some(-36.0), 10.0),
                (Some(-23.0), 60.0),
                (Some(-36.0), 10.0),
                (Some(-72.0), 10.0),
            ],
        );
        within(r.integrated, -23.0, 0.1, "I");
    }

    /// Tech 3341 case 5, and the §4.1 rule. Every block passes the gate, so the
    /// answer is a plain average — of energy: (40 s × 10^−2.6 + 20.1 s ×
    /// 10^−2.0) / 60.1 s is −23.0 LUFS. Averaging the block loudnesses
    /// instead gives (40 × −26 + 20.1 × −20) / 60.1 = −24.0, a full LU low,
    /// so an "integrated = mean of LUFS" simplification fails here.
    #[test]
    fn integrated_loudness_averages_energy_not_lufs() {
        let r = measure(
            RATE,
            &[
                (Some(-26.0), 20.0),
                (Some(-20.0), 20.1),
                (Some(-26.0), 20.0),
            ],
        );
        within(r.integrated, -23.0, 0.1, "I");
    }

    /// Five channels at different levels, with the surrounds weighted 1.41.
    #[test]
    fn tech_3341_case_6_five_channels_with_surround_weights() {
        let levels = [-28.0, -28.0, -24.0, -30.0, -30.0];
        let weights = [1.0, 1.0, 1.0, 1.41, 1.41];
        let frames = (20.0 * RATE) as usize;
        let mut buf = Vec::with_capacity(frames * 5);
        for n in 0..frames {
            let phase = (2.0 * PI * 1000.0 * n as f64 / RATE).sin();
            for db in levels {
                buf.push((amplitude(db) * phase) as f32);
            }
        }
        let mut m = LoudnessMeter::new(RATE, &weights);
        feed(&mut m, &buf, 5);
        within(m.reading().integrated, -23.0, 0.1, "I");
    }

    /// A 3 s period exactly fills the short-term window, so once full, every
    /// window holds one period and S does not move.
    #[test]
    fn tech_3341_case_9_short_term_is_steady_over_its_own_period() {
        let period = [(Some(-20.0), 1.34), (Some(-30.0), 1.66)];
        let mut m = LoudnessMeter::new(RATE, &STEREO);
        let signal = tones(RATE, 2, &period.repeat(5));
        let (head, tail) = signal.split_at((3.0 * RATE) as usize * 2);
        feed(&mut m, head, 2);
        for tick in tail.chunks(480 * 2) {
            feed(&mut m, tick, 2);
            within(m.reading().short_term, -23.0, 0.1, "S after 3 s");
        }
    }

    #[test]
    fn tech_3341_case_12_momentary_is_steady_over_its_own_period() {
        let period = [(Some(-20.0), 0.18), (Some(-30.0), 0.22)];
        let mut m = LoudnessMeter::new(RATE, &STEREO);
        let signal = tones(RATE, 2, &period.repeat(25));
        let (head, tail) = signal.split_at(RATE as usize * 2);
        feed(&mut m, head, 2);
        for tick in tail.chunks(480 * 2) {
            feed(&mut m, tick, 2);
            within(m.reading().momentary, -23.0, 0.1, "M after 1 s");
        }
    }

    /// File-based: each segment measured from a fresh reset. The tone starts
    /// at a different offset each time, which is what defeats a 100 ms grid.
    #[test]
    fn tech_3341_case_10_max_short_term_wherever_the_tone_starts() {
        for i in 0..20 {
            let r = measure(
                RATE,
                &[(None, i as f64 * 0.15), (Some(-23.0), 3.0), (None, 1.0)],
            );
            within(r.max_short_term, -23.0, 0.1, &format!("max S, segment {i}"));
        }
    }

    /// Live: one continuous signal, read after each segment.
    #[test]
    fn tech_3341_case_11_max_short_term_live() {
        let mut m = LoudnessMeter::new(RATE, &STEREO);
        for i in 0..20 {
            let level = -38.0 + i as f64;
            let pause = i as f64 * 0.15;
            let segment = tones(
                RATE,
                2,
                &[(None, pause), (Some(level), 3.0), (None, 3.0 - pause)],
            );
            feed(&mut m, &segment, 2);
            within(
                m.reading().max_short_term,
                level,
                0.1,
                &format!("max S, segment {i}"),
            );
        }
    }

    #[test]
    fn tech_3341_case_13_max_momentary_wherever_the_tone_starts() {
        for i in 0..20 {
            let r = measure(
                RATE,
                &[(None, i as f64 * 0.02), (Some(-23.0), 0.4), (None, 1.0)],
            );
            within(r.max_momentary, -23.0, 0.1, &format!("max M, segment {i}"));
        }
    }

    #[test]
    fn tech_3341_case_14_max_momentary_live() {
        let mut m = LoudnessMeter::new(RATE, &STEREO);
        for i in 0..20 {
            let level = -38.0 + i as f64;
            let pause = i as f64 * 0.02;
            let segment = tones(
                RATE,
                2,
                &[(None, pause), (Some(level), 0.4), (None, 0.4 - pause)],
            );
            feed(&mut m, &segment, 2);
            within(
                m.reading().max_momentary,
                level,
                0.1,
                &format!("max M, segment {i}"),
            );
        }
    }

    /// Tech 3341 case 1 again, at the other common rates: the derived filter
    /// and the 10 ms ticks have to give the same answer as 48 kHz.
    #[test]
    fn a_minus_23_tone_reads_minus_23_at_every_rate() {
        for rate in [44100.0, 48000.0, 96000.0] {
            let r = measure(rate, &[(Some(-23.0), 20.0)]);
            within(r.momentary, -23.0, 0.1, &format!("M at {rate}"));
            within(r.short_term, -23.0, 0.1, &format!("S at {rate}"));
            within(r.integrated, -23.0, 0.1, &format!("I at {rate}"));
        }
    }

    /// BS.1770-5 Annex 1: "If a 0 dB FS, 1 kHz (997 Hz to be exact) sine wave
    /// is applied to the left, centre, or right channel input, the indicated
    /// loudness will equal −3.01 LKFS." Also the mono case.
    #[test]
    fn a_full_scale_sine_on_one_channel_reads_minus_three_lufs() {
        let frames = (5.0 * RATE) as usize;
        let buf: Vec<f32> = (0..frames)
            .map(|n| (2.0 * PI * 997.0 * n as f64 / RATE).sin() as f32)
            .collect();
        let mut m = LoudnessMeter::new(RATE, &[1.0]);
        feed(&mut m, &buf, 1);
        let r = m.reading();
        within(r.momentary, -3.01, 0.01, "M");
        within(r.integrated, -3.01, 0.01, "I");
    }

    // --- Loudness range: Tech 3342 Table 1 ----------------------------------

    #[test]
    fn tech_3342_case_1_two_tones_ten_apart() {
        let r = measure(RATE, &[(Some(-20.0), 20.0), (Some(-30.0), 20.0)]);
        within(r.range, 10.0, 1.0, "LRA");
    }

    #[test]
    fn tech_3342_case_2_two_tones_five_apart() {
        let r = measure(RATE, &[(Some(-20.0), 20.0), (Some(-15.0), 20.0)]);
        within(r.range, 5.0, 1.0, "LRA");
    }

    /// −40 is 20 LU below −20, right on the range's relative gate.
    #[test]
    fn tech_3342_case_3_two_tones_twenty_apart() {
        let r = measure(RATE, &[(Some(-40.0), 20.0), (Some(-20.0), 20.0)]);
        within(r.range, 20.0, 1.0, "LRA");
    }

    #[test]
    fn tech_3342_case_4_five_tones() {
        let r = measure(
            RATE,
            &[
                (Some(-50.0), 20.0),
                (Some(-35.0), 20.0),
                (Some(-20.0), 20.0),
                (Some(-35.0), 20.0),
                (Some(-50.0), 20.0),
            ],
        );
        within(r.range, 15.0, 1.0, "LRA");
    }

    // --- True peak: Tech 3341 Table 1, cases 15–23 --------------------------

    /// A tone at `cycles_per_sample · fs`, tapered with a 10 ms raised-cosine
    /// fade at each end as Tech 3341 asks.
    fn true_peak_tone(rate: f64, cycles_per_sample: f64, amplitude: f64, degrees: f64) -> Vec<f32> {
        let frames = rate as usize;
        let fade = (0.01 * rate) as usize;
        let mut out = Vec::with_capacity(frames * 2);
        for n in 0..frames {
            let edge = n.min(frames - 1 - n);
            let taper = if edge < fade {
                0.5 - 0.5 * (PI * edge as f64 / fade as f64).cos()
            } else {
                1.0
            };
            let s = amplitude
                * taper
                * (2.0 * PI * cycles_per_sample * n as f64 + degrees.to_radians()).sin();
            out.push(s as f32);
            out.push(s as f32);
        }
        out
    }

    fn true_peak_of(rate: f64, buf: &[f32]) -> f64 {
        let mut m = LoudnessMeter::new(rate, &STEREO);
        feed(&mut m, buf, 2);
        m.reading().true_peak.expect("a true peak")
    }

    fn within_true_peak(got: f64, want: f64, what: &str) {
        assert!(
            got <= want + 0.2 && got >= want - 0.4,
            "{what}: {got:.3} dBTP, expected {want} +0.2/−0.4"
        );
    }

    #[test]
    fn tech_3341_cases_15_to_19_true_peak_of_tones() {
        let cases = [
            (15, 0.25, 0.50, 0.0, -6.0),
            (16, 0.25, 0.50, 45.0, -6.0),
            (17, 1.0 / 6.0, 0.50, 60.0, -6.0),
            (18, 0.125, 0.50, 67.5, -6.0),
            (19, 0.25, 1.41, 45.0, 3.0),
        ];
        for (case, f, a, phase, want) in cases {
            let got = true_peak_of(RATE, &true_peak_tone(RATE, f, a, phase));
            within_true_peak(got, want, &format!("case {case}"));
        }
    }

    /// Case 16 is the one a sample-peak meter gets wrong: at 45° no sample
    /// lands within 3 dB of the crest.
    #[test]
    fn true_peak_sees_what_the_samples_miss() {
        let buf = true_peak_tone(RATE, 0.25, 0.5, 45.0);
        let sample_peak = buf.iter().fold(0.0f64, |m, &s| m.max((s as f64).abs()));
        let got = true_peak_of(RATE, &buf);
        assert!(
            got - amplitude_to_dbfs(sample_peak) > 2.5,
            "true peak {got:.2} vs sample peak {:.2}",
            amplitude_to_dbfs(sample_peak)
        );
    }

    #[test]
    fn true_peak_holds_at_44_1_khz() {
        let got = true_peak_of(44100.0, &true_peak_tone(44100.0, 0.25, 0.5, 45.0));
        within_true_peak(got, -6.0, "fs/4 at 45° at 44.1 kHz");
    }

    /// Cases 20–23: a 0.5 FFS tone at fs/6 with one period of a 1.0 FFS tone
    /// at fs/4 spliced in, built at 4·fs, low-passed, and decimated at each of
    /// the four possible offsets.
    #[test]
    fn tech_3341_cases_20_to_23_true_peak_of_a_burst_between_samples() {
        let up = 4.0 * RATE;
        // At 192 kHz an fs/6 period is 24 samples and an fs/4 period is 16; both
        // start rising from zero, so the splice is continuous in phase.
        let mut hi = Vec::new();
        for n in 0..(24 * 400) {
            hi.push(0.5 * (2.0 * PI * n as f64 / 24.0).sin());
        }
        for n in 0..16 {
            hi.push((2.0 * PI * n as f64 / 16.0).sin());
        }
        for n in 0..(24 * 400) {
            hi.push(0.5 * (2.0 * PI * n as f64 / 24.0).sin());
        }
        let fade = (0.01 * up) as usize;
        let len = hi.len();
        for (n, s) in hi.iter_mut().enumerate() {
            let edge = n.min(len - 1 - n);
            if edge < fade {
                *s *= 0.5 - 0.5 * (PI * edge as f64 / fade as f64).cos();
            }
        }

        // Windowed-sinc anti-aliasing filter, cut off at 21 kHz.
        let taps = 1023usize;
        let cutoff = 21000.0 / up;
        let mid = (taps / 2) as f64;
        let h: Vec<f64> = (0..taps)
            .map(|i| {
                let t = i as f64 - mid;
                let sinc = if t == 0.0 {
                    2.0 * cutoff
                } else {
                    (2.0 * PI * cutoff * t).sin() / (PI * t)
                };
                let w = 0.42 - 0.5 * (2.0 * PI * i as f64 / (taps - 1) as f64).cos()
                    + 0.08 * (4.0 * PI * i as f64 / (taps - 1) as f64).cos();
                sinc * w
            })
            .collect();
        let filtered: Vec<f64> = (0..len)
            .map(|n| {
                let mut y = 0.0;
                for (k, hk) in h.iter().enumerate() {
                    let j = n as i64 + mid as i64 - k as i64;
                    if j >= 0 && (j as usize) < len {
                        y += hk * hi[j as usize];
                    }
                }
                y
            })
            .collect();

        for offset in 0..4 {
            let mut buf = Vec::new();
            for s in filtered.iter().skip(offset).step_by(4) {
                buf.push(*s as f32);
                buf.push(*s as f32);
            }
            let got = true_peak_of(RATE, &buf);
            within_true_peak(got, 0.0, &format!("case {}", 20 + offset));
        }
    }

    // --- Lifecycle ----------------------------------------------------------

    #[test]
    fn readings_appear_only_once_their_windows_are_full() {
        let mut m = LoudnessMeter::new(RATE, &STEREO);
        feed(&mut m, &tones(RATE, 2, &[(Some(-20.0), 0.39)]), 2);
        let r = m.reading();
        assert!(r.momentary.is_none() && r.integrated.is_none(), "{r:?}");

        feed(&mut m, &tones(RATE, 2, &[(Some(-20.0), 0.02)]), 2);
        let r = m.reading();
        assert!(r.momentary.is_some() && r.integrated.is_some(), "{r:?}");
        assert!(r.short_term.is_none() && r.range.is_none(), "{r:?}");

        feed(&mut m, &tones(RATE, 2, &[(Some(-20.0), 2.7)]), 2);
        let r = m.reading();
        assert!(r.short_term.is_some() && r.range.is_some(), "{r:?}");
    }

    #[test]
    fn digital_silence_reads_the_floor_not_infinity() {
        let mut m = LoudnessMeter::new(RATE, &STEREO);
        feed(&mut m, &vec![0.0f32; (5.0 * RATE) as usize * 2], 2);
        let r = m.reading();
        assert_eq!(r.momentary, Some(SILENCE_DBFS));
        assert_eq!(r.short_term, Some(SILENCE_DBFS));
        assert_eq!(r.true_peak, Some(SILENCE_DBFS));
        assert_eq!(r.integrated, None, "every block is below the absolute gate");
        assert_eq!(r.range, None);
        assert!(serde_json::to_string(&r).is_ok());
    }

    /// The reset happens mid-tick, after five seconds of something 20 LU
    /// louder. None of it may leak into the new measurement — not through the
    /// integrated loudness, not through the maxima — while M and S, which
    /// describe the signal rather than the measurement, carry straight on.
    #[test]
    fn a_reset_restarts_the_measurement_but_not_the_meter() {
        let mut m = LoudnessMeter::new(RATE, &STEREO);
        let loud = tones(RATE, 2, &[(Some(-10.0), 5.0)]);
        feed(&mut m, &loud, 2);
        feed(&mut m, &loud[..123 * 2], 2);

        m.reset();
        let r = m.reading();
        within(r.momentary, -10.0, 0.1, "M just after the reset");
        assert_eq!(r.integrated, None);
        assert_eq!(r.max_momentary, None);
        assert_eq!(r.true_peak, None);
        assert_eq!(r.elapsed_seconds, 0.0);

        feed(&mut m, &tones(RATE, 2, &[(Some(-30.0), 10.0)]), 2);
        let r = m.reading();
        within(r.integrated, -30.0, 0.1, "I");
        within(r.max_momentary, -30.0, 0.1, "max M");
        within(r.max_short_term, -30.0, 0.1, "max S");
        within(r.true_peak, -30.0, 0.2, "true peak");
        assert!((r.elapsed_seconds - 10.0).abs() < 1e-9);
    }

    #[test]
    fn reconfiguring_to_the_same_thing_keeps_the_measurement() {
        let mut m = LoudnessMeter::new(RATE, &STEREO);
        feed(&mut m, &tones(RATE, 2, &[(Some(-20.0), 2.0)]), 2);
        m.reconfigure(RATE, &STEREO);
        assert!(m.reading().integrated.is_some());
        m.reconfigure(96000.0, &STEREO);
        assert!(m.reading().integrated.is_none() && m.reading().momentary.is_none());
    }

    /// The histogram against the algorithm it stands in for: every block kept,
    /// gated exactly, sorted exactly (Tech 3342 §5's MATLAB, transcribed).
    #[test]
    fn the_histogram_agrees_with_an_exact_gate() {
        // A deterministic spread of block loudnesses: a loud cluster, a quiet
        // one straddling both relative gates, and a tail below them.
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed >> 11) as f64 / (1u64 << 53) as f64
        };
        let lufs: Vec<f64> = (0..60_000)
            .map(|i| match i % 3 {
                0 => -18.0 - 8.0 * next(),
                1 => -30.0 - 12.0 * next(),
                _ => -45.0 - 24.0 * next(),
            })
            .collect();
        let energy = |l: f64| 10f64.powf((l - LOUDNESS_OFFSET) / 10.0);

        let mut hist = Histogram::new();
        for &l in &lufs {
            hist.add(l, energy(l));
        }

        let mean =
            |set: &[f64]| loudness(set.iter().map(|&l| energy(l)).sum::<f64>() / set.len() as f64);
        let gate = mean(&lufs) + RELATIVE_GATE;
        let gated: Vec<f64> = lufs.iter().copied().filter(|&l| l > gate).collect();
        let exact_integrated = mean(&gated);

        let gate = mean(&lufs) + RANGE_RELATIVE_GATE;
        let mut kept: Vec<f64> = lufs.iter().copied().filter(|&l| l >= gate).collect();
        kept.sort_by(|a, b| a.total_cmp(b));
        let n = kept.len();
        let at = |p: f64| kept[((n - 1) as f64 * p + 1.0).round() as usize - 1];
        let exact_range = at(RANGE_HIGH_PERCENTILE) - at(RANGE_LOW_PERCENTILE);

        let integrated = integrated_from(&hist).unwrap();
        let range = range_from(&hist).unwrap();
        assert!(
            (integrated - exact_integrated).abs() < 0.005,
            "integrated {integrated:.4} vs exact {exact_integrated:.4}"
        );
        assert!(
            (range - exact_range).abs() < 0.02,
            "range {range:.4} vs exact {exact_range:.4}"
        );
    }

    /// What crosses to the UI is camelCase, and a missing reading is `null`,
    /// not an absent key — `types.ts` mirrors it as `number | null`.
    #[test]
    fn the_reading_serialises_in_camel_case() {
        let mut m = LoudnessMeter::new(RATE, &STEREO);
        feed(&mut m, &tones(RATE, 2, &[(Some(-20.0), 0.5)]), 2);
        let v = serde_json::to_value(m.reading()).unwrap();
        let obj = v.as_object().unwrap();
        for key in [
            "momentary",
            "shortTerm",
            "integrated",
            "range",
            "maxMomentary",
            "maxShortTerm",
            "truePeak",
            "elapsedSeconds",
        ] {
            assert!(obj.contains_key(key), "{key} missing from {v}");
        }
        assert!(obj["shortTerm"].is_null());
        assert_eq!(obj.len(), 8, "{v}");
    }
}
