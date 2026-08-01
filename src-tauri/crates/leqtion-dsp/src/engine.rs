//! The measurement engine: one place where samples become every number the UI
//! shows.
//!
//! The engine owns the analysis state and nothing else — no audio device, no
//! threads, no timers. Samples are pushed in as they arrive, frames are pulled
//! out whenever the display wants one, and the two rates are unrelated. That
//! separation is what lets the whole measurement chain be tested against
//! synthetic signals, and it is why a slow or stalled UI cannot corrupt an LEQ:
//! the integration happens on `push`, not on `frame`.
//!
//! ```text
//!   interleaved f32 ─▶ fold to mono ─▶ ┬─▶ peak / clip
//!                                      ├─▶ spectrum (unweighted) ─▶ bands
//!                                      └─▶ A / C / Z filters ─▶ ┬─▶ SPL detectors
//!                                                               └─▶ LEQ accumulators
//! ```

use serde::{Deserialize, Serialize};

use crate::bands::BandPlan;
use crate::calibration::{Calibration, CalibrationRun, CalibrationSample, CalibrationStatus, CalibrationTarget};
use crate::history::{History, HistoryConfig, HistoryPoint, SeriesInfo, SeriesKind};
use crate::leq::{LeqAccumulator, LeqReading, LeqSpec};
use crate::spectrum::{SpectrumAnalyser, SpectrumConfig};
use crate::spl::{
    amplitude_to_dbfs, mean_square_to_dbfs, LevelDetector, MinMax, PeakTracker, TimeWeighting,
    SILENCE_DBFS,
};
use crate::weighting::{Weighting, WeightingFilter};

/// Which input channel the measurement comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ChannelSelect {
    /// One channel, counted from zero.
    Channel { index: usize },
    /// The mean of every channel.
    ///
    /// The *mean*, not the sum: two channels carrying the same mono microphone
    /// must read the same level as one, and a sum would put them 6 dB high.
    Mix,
}

impl Default for ChannelSelect {
    fn default() -> Self {
        ChannelSelect::Channel { index: 0 }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineConfig {
    pub spectrum: SpectrumConfig,
    /// Time weighting for the SPL readouts. LEQ ignores it, by definition.
    pub time_weighting: TimeWeighting,
    pub channel: ChannelSelect,
    pub leqs: Vec<LeqSpec>,
    /// How the level history is recorded. Its own settings rather than the
    /// chart tile's, because the history exists whether a chart does or not.
    #[serde(default)]
    pub history: HistoryConfig,
}

impl Default for EngineConfig {
    fn default() -> Self {
        EngineConfig {
            spectrum: SpectrumConfig::default(),
            time_weighting: TimeWeighting::Fast,
            channel: ChannelSelect::default(),
            leqs: Vec::new(),
            history: HistoryConfig::default(),
        }
    }
}

/// One weighted signal path: filter, detectors, min/max and peak.
struct Path {
    weighting: Weighting,
    filter: WeightingFilter,
    /// One detector per time weighting, in [`TimeWeighting::ALL`] order.
    ///
    /// All three run all the time, not just the one on the readout. The history
    /// chart offers Fast *and* Slow as separate traces, and a detector that only
    /// existed while it was selected would draw a line starting from whenever
    /// the user picked it — with the earlier part of the same measurement
    /// missing for no reason the chart could explain. Three one-pole filters per
    /// weighting is nothing next to the weighting filter they share.
    detectors: [LevelDetector; 3],
    minmax: MinMax,
    peak: PeakTracker,
    scratch: Vec<f32>,
    /// Mean square of the most recent block, for the LEQ accumulators.
    block_ms: f64,
}

impl Path {
    fn new(weighting: Weighting, sample_rate: f64) -> Self {
        Path {
            weighting,
            filter: WeightingFilter::new(weighting, sample_rate),
            detectors: TimeWeighting::ALL.map(|tw| LevelDetector::new(tw, sample_rate)),
            minmax: MinMax::default(),
            peak: PeakTracker::default(),
            scratch: Vec::new(),
            block_ms: 0.0,
        }
    }

    /// The detector for one time weighting.
    fn detector(&self, tw: TimeWeighting) -> &LevelDetector {
        let i = TimeWeighting::ALL.iter().position(|&t| t == tw).unwrap_or(0);
        &self.detectors[i]
    }

    fn push(&mut self, mono: &[f32], in_force: TimeWeighting) {
        self.scratch.clear();
        self.scratch.extend_from_slice(mono);
        self.filter.process_block(&mut self.scratch);

        self.peak.push(&self.scratch);
        for d in &mut self.detectors {
            d.push(&self.scratch);
        }
        // Lmax and Lmin follow the weighting on the readout, which is what a
        // meter's max and min mean. The other detectors are for the history.
        self.minmax.push(self.detector(in_force).mean_square());

        let mut sum = 0.0f64;
        for &x in &self.scratch {
            sum += (x as f64) * (x as f64);
        }
        self.block_ms = if self.scratch.is_empty() {
            0.0
        } else {
            sum / self.scratch.len() as f64
        };
    }

    /// Clear the *statistics* but leave the filter and detector running.
    ///
    /// Deliberately not a full reset. Resetting the weighting filter's state
    /// mid-signal puts a step through it and a transient into the level;
    /// resetting the detector would drop the live SPL to silence and let it
    /// climb back, which is not what a meter does when you clear its Lmax. Only
    /// the accumulated history goes.
    fn reset_statistics(&mut self) {
        self.minmax.reset();
        self.peak.reset();
    }
}

/// Extract one mono signal from an interleaved block.
///
/// Public because the transfer function needs the *same* measurement signal the
/// meter is using, and two implementations of "which channel" would eventually
/// disagree — at which point the RTA and the transfer function would be looking
/// at different microphones and nothing on screen would say so.
///
/// `out` is cleared and refilled; passing the same buffer back avoids
/// allocating on the analysis thread.
pub fn fold_channels(samples: &[f32], channels: usize, select: ChannelSelect, out: &mut Vec<f32>) {
    out.clear();
    if channels == 0 || samples.is_empty() {
        return;
    }
    let frames = samples.len() / channels;
    out.reserve(frames);

    match select {
        ChannelSelect::Channel { index } => {
            let c = index.min(channels - 1);
            for f in 0..frames {
                out.push(samples[f * channels + c]);
            }
        }
        ChannelSelect::Mix => {
            let inv = 1.0 / channels as f32;
            for f in 0..frames {
                let mut sum = 0.0f32;
                for c in 0..channels {
                    sum += samples[f * channels + c];
                }
                out.push(sum * inv);
            }
        }
    }
}

/// Every series the history records, in the order values are fed to it.
///
/// One function rather than two lists, because [`History::push`] takes values
/// positionally: a reported order that disagreed with the pushed order would
/// label every trace with its neighbour's numbers, and nothing on screen would
/// look wrong.
fn history_series(leqs: &[LeqSpec]) -> Vec<SeriesKind> {
    let mut series = Vec::new();
    for &weighting in &Weighting::ALL {
        for &time_weighting in &TimeWeighting::ALL {
            series.push(SeriesKind::Spl {
                weighting,
                time_weighting,
            });
        }
        series.push(SeriesKind::Peak { weighting });
    }
    for spec in leqs {
        series.push(SeriesKind::Leq {
            id: spec.id.clone(),
        });
    }
    series
}

/// A level readout for one weighting.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SplReading {
    pub weighting: Weighting,
    /// Time-weighted level — the moving number on the meter.
    pub level: f64,
    /// Highest and lowest time-weighted level since the last reset.
    pub max: f64,
    pub min: f64,
    /// Highest sample peak since the last reset.
    pub peak: f64,
}

/// Everything the UI needs for one repaint.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Frame {
    pub sample_rate: f64,
    /// Bumped whenever the band table changes, so the UI knows to refetch it
    /// rather than shipping several hundred band labels thirty times a second.
    pub plan_revision: u64,
    /// True when levels are dB SPL. False means they are dBFS and the UI must
    /// say so — an uncalibrated number presented as an SPL is the single most
    /// damaging thing this app could do.
    pub calibrated: bool,
    pub bands_db: Vec<f32>,
    pub peaks_db: Vec<f32>,
    pub spl: Vec<SplReading>,
    pub leqs: Vec<LeqReading>,
    /// Time weighting in force for `spl`.
    pub time_weighting: TimeWeighting,
    /// Strongest frequency component, Hz. Useful on its own, and what the
    /// calibration screen checks the calibrator against.
    pub dominant_hz: Option<f64>,
    /// Unweighted sample peak of the raw input, always in dBFS regardless of
    /// calibration — this is a converter headroom figure, not a sound level.
    pub input_peak_dbfs: f64,
    pub clipped: bool,
    /// Seconds of audio the engine has processed since the last reset.
    pub elapsed_seconds: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub calibration: Option<CalibrationStatus>,
}

pub struct Engine {
    config: EngineConfig,
    sample_rate: f64,
    spectrum: SpectrumAnalyser,
    paths: Vec<Path>,
    leqs: Vec<LeqAccumulator>,
    input_peak: PeakTracker,
    calibration: Option<Calibration>,
    run: Option<CalibrationRun>,
    mono: Vec<f32>,
    elapsed: f64,
    plan_revision: u64,
    history: History,
    /// Reused buffer of one value per series, so the per-block feed into the
    /// history does not allocate on the analysis thread.
    history_values: Vec<f64>,
}

impl Engine {
    pub fn new(config: EngineConfig, sample_rate: f64) -> Self {
        let spectrum = SpectrumAnalyser::new(config.spectrum, sample_rate);
        let paths = Weighting::ALL
            .iter()
            .map(|&w| Path::new(w, sample_rate))
            .collect();
        let leqs = config
            .leqs
            .iter()
            .cloned()
            .map(LeqAccumulator::new)
            .collect();
        let config_history = config.history;
        let series = history_series(&config.leqs);

        Engine {
            config,
            sample_rate,
            spectrum,
            paths,
            leqs,
            input_peak: PeakTracker::default(),
            calibration: None,
            run: None,
            mono: Vec::new(),
            elapsed: 0.0,
            plan_revision: 1,
            history: History::new(config_history, series),
            history_values: Vec::new(),
        }
    }

    pub fn config(&self) -> &EngineConfig {
        &self.config
    }

    pub fn sample_rate(&self) -> f64 {
        self.sample_rate
    }

    pub fn plan(&self) -> &BandPlan {
        self.spectrum.plan()
    }

    pub fn plan_revision(&self) -> u64 {
        self.plan_revision
    }

    pub fn calibration(&self) -> Option<&Calibration> {
        self.calibration.as_ref()
    }

    pub fn set_calibration(&mut self, cal: Option<Calibration>) {
        self.calibration = cal;
    }

    /// dB offset applied to every level. Zero, and flagged uncalibrated, when
    /// no calibration is loaded.
    fn offset_db(&self) -> f64 {
        self.calibration.as_ref().map(|c| c.offset_db).unwrap_or(0.0)
    }

    fn to_level(&self, dbfs: f64) -> f64 {
        if dbfs <= SILENCE_DBFS {
            return SILENCE_DBFS;
        }
        dbfs + self.offset_db()
    }

    /// Replace the configuration.
    ///
    /// Sample rate is passed alongside because a device change can move it, and
    /// every filter and detector coefficient depends on it.
    pub fn reconfigure(&mut self, config: EngineConfig, sample_rate: f64) {
        let rate_changed = (sample_rate - self.sample_rate).abs() > f64::EPSILON;
        let bands_before = self.spectrum.plan().bands.len();

        self.spectrum.reconfigure(config.spectrum, sample_rate);
        if self.spectrum.plan().bands.len() != bands_before
            || config.spectrum.fraction != self.config.spectrum.fraction
        {
            self.plan_revision += 1;
        }

        if rate_changed {
            self.paths = Weighting::ALL
                .iter()
                .map(|&w| Path::new(w, sample_rate))
                .collect();
        }

        // Reconcile the LEQ set by id, so an unrelated edit does not restart
        // every other LEQ on the screen.
        let mut kept: Vec<LeqAccumulator> = Vec::with_capacity(config.leqs.len());
        for spec in &config.leqs {
            match self.leqs.iter().position(|a| a.spec().id == spec.id) {
                Some(i) => {
                    let mut acc = self.leqs.remove(i);
                    acc.reconfigure(spec.clone());
                    kept.push(acc);
                }
                None => kept.push(LeqAccumulator::new(spec.clone())),
            }
        }
        self.leqs = kept;

        self.history.set_series(history_series(&config.leqs));
        if config.history != self.config.history {
            self.history.reconfigure(config.history);
        }

        self.sample_rate = sample_rate;
        self.config = config;
    }

    /// Feed interleaved input.
    ///
    /// Returns true when this block completed a history interval, which is the
    /// signal a logger writes a row on. Deriving that from a clock instead
    /// would put the log and the chart on different time bases, and a log whose
    /// rows do not line up with the trace they came from is worse than no log.
    pub fn push_interleaved(&mut self, samples: &[f32], channels: usize) -> bool {
        if channels == 0 || samples.is_empty() {
            return false;
        }
        let frames = samples.len() / channels;
        if frames == 0 {
            return false;
        }

        fold_channels(samples, channels, self.config.channel, &mut self.mono);

        let seconds = frames as f64 / self.sample_rate;
        self.elapsed += seconds;

        self.input_peak.push(&self.mono);
        // Take the mono buffer out so the paths can be borrowed mutably
        // alongside it. It is put straight back; nothing else touches it.
        let mono = std::mem::take(&mut self.mono);
        self.spectrum.push(&mono);
        let in_force = self.config.time_weighting;
        for p in &mut self.paths {
            p.push(&mono, in_force);
        }

        for acc in &mut self.leqs {
            let w = acc.spec().weighting;
            if let Some(p) = self.paths.iter().find(|p| p.weighting == w) {
                acc.push(p.block_ms, seconds);
            }
        }

        // Fed after the paths and the LEQ accumulators, so a point covers the
        // block that has just been analysed rather than the one before it.
        self.history_values.clear();
        for p in &self.paths {
            for &tw in &TimeWeighting::ALL {
                self.history_values
                    .push(self.to_level(p.detector(tw).level_dbfs()));
            }
            self.history_values
                .push(self.to_level(amplitude_to_dbfs(p.peak.peak())));
        }
        for acc in &self.leqs {
            self.history_values.push(self.to_level(acc.leq_dbfs()));
        }
        let values = std::mem::take(&mut self.history_values);
        let completed = self.history.push(seconds, &values);
        self.history_values = values;

        if let Some(run) = &mut self.run {
            let z = self
                .paths
                .iter()
                .find(|p| p.weighting == Weighting::Z)
                .expect("Z path always exists");
            run.push(CalibrationSample {
                level_dbfs: mean_square_to_dbfs(z.block_ms),
                dominant_hz: self.spectrum.dominant_hz(),
                peak: self.input_peak.peak(),
                seconds,
            });
        }

        self.mono = mono;
        completed
    }

    /// Snapshot for one repaint.
    pub fn frame(&self) -> Frame {
        let calibrated = self.calibration.is_some();

        let spl = self
            .paths
            .iter()
            .map(|p| SplReading {
                weighting: p.weighting,
                level: self.to_level(p.detector(self.config.time_weighting).level_dbfs()),
                max: self.to_level(p.minmax.max_dbfs()),
                min: self.to_level(p.minmax.min_dbfs()),
                peak: self.to_level(p.peak.peak_dbfs()),
            })
            .collect();

        let leqs = self
            .leqs
            .iter()
            .map(|a| LeqReading {
                id: a.spec().id.clone(),
                label: a.spec().display_label(),
                weighting: a.spec().weighting,
                value: self.to_level(a.leq_dbfs()),
                calibrated,
                elapsed_seconds: a.elapsed_seconds(),
                integrated_seconds: a.integrated_seconds(),
                fill: a.fill(),
            })
            .collect();

        let offset = self.offset_db();
        let bands_db = if calibrated {
            self.spectrum
                .bands_db()
                .iter()
                .map(|&v| v + offset as f32)
                .collect()
        } else {
            self.spectrum.bands_db().to_vec()
        };
        let peaks_db = if calibrated {
            self.spectrum
                .peaks_db()
                .iter()
                .map(|&v| if v.is_finite() { v + offset as f32 } else { v })
                .collect()
        } else {
            self.spectrum.peaks_db().to_vec()
        };

        Frame {
            sample_rate: self.sample_rate,
            plan_revision: self.plan_revision,
            calibrated,
            bands_db,
            peaks_db,
            spl,
            leqs,
            time_weighting: self.config.time_weighting,
            dominant_hz: self.spectrum.dominant_hz(),
            input_peak_dbfs: amplitude_to_dbfs(self.input_peak.peak()),
            clipped: self.input_peak.clipped(),
            elapsed_seconds: self.elapsed,
            calibration: self.run.as_ref().map(|r| r.status()),
        }
    }

    /// Clear the running measurement: LEQs, min/max, peaks and the spectrum
    /// average. Does not touch the calibration, which survives a reset — that
    /// is a property of the microphone, not of the measurement.
    pub fn reset_measurement(&mut self) {
        for p in &mut self.paths {
            p.reset_statistics();
        }
        for a in &mut self.leqs {
            a.reset();
        }
        self.input_peak.reset();
        self.spectrum.reset_average();
        self.spectrum.reset_peaks();
        self.history.reset();
        self.elapsed = 0.0;
    }

    /// Every series the history is recording, with labels for the UI.
    ///
    /// LEQ series are relabelled from their spec on the way out: the history
    /// only knows a LEQ by its id, so on its own it would offer "LEQ leq-a" in a
    /// chart's series menu, which is a key rather than a name. The engine has
    /// the spec, so it is the one place that can say "LAeq,10s".
    pub fn history_series(&self) -> Vec<SeriesInfo> {
        let mut series = self.history.series();
        for info in &mut series {
            if let SeriesKind::Leq { id } = &info.kind {
                if let Some(acc) = self.leqs.iter().find(|a| &a.spec().id == id) {
                    info.label = acc.spec().display_label();
                }
            }
        }
        series
    }

    /// The last `seconds` of one series, at most `max_points` of them.
    pub fn history_view(&self, id: &str, seconds: f64, max_points: usize) -> Vec<HistoryPoint> {
        self.history.view(id, seconds, max_points)
    }

    /// The most recently completed point of every series.
    ///
    /// This is what a logger writes: one row per interval, every series at
    /// once, taken from the same points the chart draws rather than sampled
    /// separately — two samplings of one measurement would disagree, and the
    /// log is the copy someone keeps.
    pub fn history_latest(&self) -> Vec<(SeriesInfo, HistoryPoint)> {
        self.history
            .series()
            .into_iter()
            .filter_map(|info| self.history.last(&info.id).map(|p| (info, p)))
            .collect()
    }

    pub fn history_config(&self) -> HistoryConfig {
        self.history.config()
    }

    /// Seconds of history recorded since the last reset.
    pub fn history_elapsed(&self) -> f64 {
        self.history.elapsed()
    }

    pub fn reset_peak_hold(&mut self) {
        self.spectrum.reset_peaks();
    }

    pub fn begin_calibration(&mut self, target: CalibrationTarget) {
        self.run = Some(CalibrationRun::new(target));
    }

    pub fn calibration_status(&self) -> Option<CalibrationStatus> {
        self.run.as_ref().map(|r| r.status())
    }

    /// Accept the running calibration. Returns it, or `None` if the run is not
    /// yet acceptable — the reason is in [`Engine::calibration_status`].
    pub fn accept_calibration(&mut self) -> Option<Calibration> {
        let cal = self.run.as_ref()?.accept()?;
        self.run = None;
        self.calibration = Some(cal.clone());
        Some(cal)
    }

    pub fn cancel_calibration(&mut self) {
        self.run = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bands::Fraction;
    use crate::leq::LeqWindow;

    const RATE: f64 = 48000.0;

    fn engine(leqs: Vec<LeqSpec>) -> Engine {
        let mut cfg = EngineConfig {
            leqs,
            ..EngineConfig::default()
        };
        cfg.spectrum.fraction = Fraction::Third;
        cfg.spectrum.fft_size = 8192;
        Engine::new(cfg, RATE)
    }

    fn sine(hz: f64, amplitude: f64, seconds: f64, channels: usize) -> Vec<f32> {
        let frames = (seconds * RATE) as usize;
        let mut v = Vec::with_capacity(frames * channels);
        for i in 0..frames {
            let s = (amplitude * (2.0 * std::f64::consts::PI * hz * i as f64 / RATE).sin()) as f32;
            for _ in 0..channels {
                v.push(s);
            }
        }
        v
    }

    /// The history must describe the same measurement the readout does. This is
    /// the failure that would be invisible: the values are pushed positionally,
    /// so a series list that drifted out of step with the push order would put
    /// every trace under its neighbour's name and still look plausible.
    #[test]
    fn the_history_agrees_with_the_live_readout() {
        let mut e = engine(vec![]);
        let mut cfg = e.config().clone();
        cfg.history.interval_seconds = 0.5;
        e.reconfigure(cfg, RATE);

        // Two seconds of a steady tone: four complete intervals.
        for _ in 0..20 {
            e.push_interleaved(&sine(1000.0, 0.1, 0.1, 1), 1);
        }

        let frame = e.frame();
        let live = frame
            .spl
            .iter()
            .find(|s| s.weighting == Weighting::A)
            .expect("an A reading")
            .level;

        let points = e.history_view("spl:a:f", 10.0, 100);
        assert!(points.len() >= 3, "got {} points", points.len());

        let last = points.last().unwrap();
        assert!(
            live >= last.min - 0.5 && live <= last.max + 0.5,
            "live LAF {live} is outside the last history point {last:?}"
        );
    }

    /// A chart's series menu must offer "LAeq,10s", not the key "leq:leq-a".
    #[test]
    fn a_leq_series_is_named_after_the_leq_not_its_id() {
        let e = engine(vec![leq(
            "leq-a",
            Weighting::A,
            LeqWindow::Sliding { seconds: 10.0 },
        )]);
        let info = e
            .history_series()
            .into_iter()
            .find(|s| s.id == "leq:leq-a")
            .expect("the LEQ series exists");
        assert_eq!(info.label, "LAeq,10s");
    }

    #[test]
    fn every_weighting_and_time_weighting_is_recorded() {
        let e = engine(vec![leq("one", Weighting::A, LeqWindow::Elapsed)]);
        let ids: Vec<String> = e.history_series().into_iter().map(|s| s.id).collect();

        // Three frequency weightings, each with three time weightings and a
        // peak, plus the one LEQ.
        assert_eq!(ids.len(), 3 * 4 + 1, "{ids:?}");
        for id in ["spl:a:f", "spl:a:s", "spl:a:i", "peak:a", "leq:one"] {
            assert!(ids.contains(&id.to_string()), "{id} missing from {ids:?}");
        }
    }

    /// Slow responds later than Fast, so a tone that has just started reads
    /// lower on Slow. If both traces came from one detector they would be equal,
    /// which is exactly the bug the separate detectors exist to prevent.
    #[test]
    fn fast_and_slow_are_genuinely_different_traces() {
        let mut e = engine(vec![]);
        let mut cfg = e.config().clone();
        cfg.history.interval_seconds = 0.1;
        e.reconfigure(cfg, RATE);

        e.push_interleaved(&sine(1000.0, 0.5, 0.2, 1), 1);

        let fast = e.history_view("spl:a:f", 10.0, 100);
        let slow = e.history_view("spl:a:s", 10.0, 100);
        assert!(!fast.is_empty() && !slow.is_empty());
        assert!(
            fast.last().unwrap().mean > slow.last().unwrap().mean + 1.0,
            "fast {:?} should lead slow {:?} on a rising tone",
            fast.last(),
            slow.last()
        );
    }

    #[test]
    fn resetting_the_measurement_clears_the_history() {
        let mut e = engine(vec![]);
        for _ in 0..20 {
            e.push_interleaved(&sine(1000.0, 0.1, 0.1, 1), 1);
        }
        assert!(!e.history_view("spl:a:f", 10.0, 100).is_empty());

        e.reset_measurement();
        assert!(e.history_view("spl:a:f", 10.0, 100).is_empty());
        assert_eq!(e.history_elapsed(), 0.0);
    }

    fn leq(id: &str, weighting: Weighting, window: LeqWindow) -> LeqSpec {
        LeqSpec {
            id: id.into(),
            label: String::new(),
            weighting,
            window,
        }
    }

    #[test]
    fn uncalibrated_frames_say_so_and_report_dbfs() {
        let mut e = engine(vec![]);
        e.push_interleaved(&sine(1000.0, 1.0, 1.0, 1), 1);
        let f = e.frame();
        assert!(!f.calibrated);
        let z = f.spl.iter().find(|s| s.weighting == Weighting::Z).unwrap();
        assert!(z.level.abs() < 0.1, "expected 0 dBFS, got {}", z.level);
    }

    #[test]
    fn calibration_shifts_every_level_by_the_same_offset() {
        let mut e = engine(vec![leq("a", Weighting::A, LeqWindow::Elapsed)]);
        e.push_interleaved(&sine(1000.0, 0.05, 2.0, 1), 1);
        let before = e.frame();

        e.set_calibration(Some(Calibration::new(
            CalibrationTarget::default(),
            -26.0,
        )));
        let after = e.frame();

        let offset = 120.0;
        for (b, a) in before.spl.iter().zip(after.spl.iter()) {
            assert!((a.level - b.level - offset).abs() < 1e-9);
        }
        assert!((after.leqs[0].value - before.leqs[0].value - offset).abs() < 1e-9);
        for (b, a) in before.bands_db.iter().zip(after.bands_db.iter()) {
            assert!((a - b - offset as f32).abs() < 1e-3);
        }
        assert!(after.calibrated);
    }

    /// A 1 kHz tone reads the same under A, C and Z, because all three
    /// weightings are 0 dB at 1 kHz by definition. If this fails, the filters
    /// are not normalised where they should be.
    #[test]
    fn one_kilohertz_reads_the_same_under_every_weighting() {
        let mut e = engine(vec![]);
        e.push_interleaved(&sine(1000.0, 0.5, 3.0, 1), 1);
        let f = e.frame();
        let a = f.spl.iter().find(|s| s.weighting == Weighting::A).unwrap();
        let c = f.spl.iter().find(|s| s.weighting == Weighting::C).unwrap();
        let z = f.spl.iter().find(|s| s.weighting == Weighting::Z).unwrap();
        assert!((a.level - z.level).abs() < 0.05, "A {} vs Z {}", a.level, z.level);
        assert!((c.level - z.level).abs() < 0.05, "C {} vs Z {}", c.level, z.level);
    }

    /// At 100 Hz, A-weighting is 19.1 dB down and C is 0.3 dB down. This is the
    /// end-to-end check that the weighting filters are actually in the signal
    /// path and the right way round.
    #[test]
    fn low_frequency_is_attenuated_by_a_more_than_c() {
        let mut e = engine(vec![]);
        e.push_interleaved(&sine(100.0, 0.5, 4.0, 1), 1);
        let f = e.frame();
        let a = f.spl.iter().find(|s| s.weighting == Weighting::A).unwrap();
        let c = f.spl.iter().find(|s| s.weighting == Weighting::C).unwrap();
        let z = f.spl.iter().find(|s| s.weighting == Weighting::Z).unwrap();

        let a_drop = z.level - a.level;
        let c_drop = z.level - c.level;
        assert!(
            (a_drop - 19.1).abs() < 0.5,
            "A-weighting took off {a_drop:.2} dB at 100 Hz, expected 19.1"
        );
        assert!(
            (c_drop - 0.3).abs() < 0.2,
            "C-weighting took off {c_drop:.2} dB at 100 Hz, expected 0.3"
        );
    }

    #[test]
    fn leq_of_a_steady_tone_equals_its_level() {
        let mut e = engine(vec![leq("z", Weighting::Z, LeqWindow::Elapsed)]);
        e.push_interleaved(&sine(1000.0, 0.5, 5.0, 1), 1);
        let f = e.frame();
        let z = f.spl.iter().find(|s| s.weighting == Weighting::Z).unwrap();
        assert!(
            (f.leqs[0].value - z.level).abs() < 0.1,
            "LEQ {} vs level {}",
            f.leqs[0].value,
            z.level
        );
    }

    #[test]
    fn several_leqs_run_independently() {
        let mut e = engine(vec![
            leq("short", Weighting::A, LeqWindow::Sliding { seconds: 1.0 }),
            leq("long", Weighting::A, LeqWindow::Elapsed),
        ]);
        // Loud for a second, then quiet for three.
        e.push_interleaved(&sine(1000.0, 1.0, 1.0, 1), 1);
        e.push_interleaved(&sine(1000.0, 0.001, 3.0, 1), 1);
        let f = e.frame();
        let short = f.leqs.iter().find(|l| l.id == "short").unwrap();
        let long = f.leqs.iter().find(|l| l.id == "long").unwrap();
        assert!(
            long.value > short.value + 20.0,
            "elapsed LEQ {} should still remember the loud part; sliding was {}",
            long.value,
            short.value
        );
        assert!((long.elapsed_seconds - 4.0).abs() < 0.01);
    }

    #[test]
    fn editing_one_leq_leaves_the_others_running() {
        let mut e = engine(vec![
            leq("a", Weighting::A, LeqWindow::Elapsed),
            leq("b", Weighting::C, LeqWindow::Elapsed),
        ]);
        e.push_interleaved(&sine(1000.0, 0.5, 2.0, 1), 1);
        let before = e.frame();

        let mut cfg = e.config().clone();
        cfg.leqs[1] = leq("b", Weighting::Z, LeqWindow::Elapsed);
        e.reconfigure(cfg, RATE);
        let after = e.frame();

        let a_before = before.leqs.iter().find(|l| l.id == "a").unwrap();
        let a_after = after.leqs.iter().find(|l| l.id == "a").unwrap();
        assert!(
            (a_after.elapsed_seconds - a_before.elapsed_seconds).abs() < 1e-9,
            "editing b restarted a"
        );
        let b_after = after.leqs.iter().find(|l| l.id == "b").unwrap();
        assert_eq!(b_after.elapsed_seconds, 0.0, "b should have restarted");
    }

    #[test]
    fn a_removed_leq_disappears_and_a_new_one_starts_clean() {
        let mut e = engine(vec![leq("a", Weighting::A, LeqWindow::Elapsed)]);
        e.push_interleaved(&sine(1000.0, 0.5, 1.0, 1), 1);

        let mut cfg = e.config().clone();
        cfg.leqs = vec![leq("new", Weighting::C, LeqWindow::Elapsed)];
        e.reconfigure(cfg, RATE);
        let f = e.frame();
        assert_eq!(f.leqs.len(), 1);
        assert_eq!(f.leqs[0].id, "new");
        assert_eq!(f.leqs[0].elapsed_seconds, 0.0);
    }

    #[test]
    fn mix_of_identical_channels_reads_the_same_as_one() {
        let mut mono = engine(vec![]);
        mono.push_interleaved(&sine(1000.0, 0.5, 1.0, 1), 1);

        let mut cfg = EngineConfig {
            channel: ChannelSelect::Mix,
            ..EngineConfig::default()
        };
        cfg.spectrum.fraction = Fraction::Third;
        cfg.spectrum.fft_size = 8192;
        let mut stereo = Engine::new(cfg, RATE);
        stereo.push_interleaved(&sine(1000.0, 0.5, 1.0, 2), 2);

        let a = mono.frame().spl[2].level;
        let b = stereo.frame().spl[2].level;
        assert!((a - b).abs() < 0.01, "mono {a} vs mixed stereo {b}");
    }

    #[test]
    fn selecting_a_channel_picks_that_channel() {
        // Left silent, right at full scale.
        let frames = 48000;
        let mut buf = Vec::with_capacity(frames * 2);
        for i in 0..frames {
            buf.push(0.0f32);
            buf.push((2.0 * std::f64::consts::PI * 1000.0 * i as f64 / RATE).sin() as f32);
        }

        let mut cfg = EngineConfig::default();
        cfg.spectrum.fft_size = 8192;
        cfg.channel = ChannelSelect::Channel { index: 1 };
        let mut right = Engine::new(cfg.clone(), RATE);
        right.push_interleaved(&buf, 2);

        cfg.channel = ChannelSelect::Channel { index: 0 };
        let mut left = Engine::new(cfg, RATE);
        left.push_interleaved(&buf, 2);

        assert!(right.frame().spl[2].level > -1.0);
        assert!(left.frame().spl[2].level < -100.0);
    }

    #[test]
    fn an_out_of_range_channel_falls_back_to_the_last_one() {
        let mut cfg = EngineConfig {
            channel: ChannelSelect::Channel { index: 7 },
            ..EngineConfig::default()
        };
        cfg.spectrum.fft_size = 8192;
        let mut e = Engine::new(cfg, RATE);
        e.push_interleaved(&sine(1000.0, 1.0, 0.5, 2), 2);
        assert!(e.frame().spl[2].level > -1.0);
    }

    #[test]
    fn clipping_is_reported() {
        let mut e = engine(vec![]);
        e.push_interleaved(&sine(1000.0, 0.5, 0.2, 1), 1);
        assert!(!e.frame().clipped);
        e.push_interleaved(&[1.0, -1.0, 1.0], 1);
        let f = e.frame();
        assert!(f.clipped);
        assert!(f.input_peak_dbfs.abs() < 1e-9);
    }

    #[test]
    fn reset_clears_the_measurement_but_keeps_the_calibration() {
        let mut e = engine(vec![leq("a", Weighting::A, LeqWindow::Elapsed)]);
        let cal = Calibration::new(CalibrationTarget::default(), -26.0);
        e.set_calibration(Some(cal));
        e.push_interleaved(&sine(1000.0, 1.0, 2.0, 1), 1);

        e.reset_measurement();
        let f = e.frame();
        assert!(f.calibrated, "reset must not throw away the calibration");
        assert_eq!(f.elapsed_seconds, 0.0);
        assert_eq!(f.leqs[0].elapsed_seconds, 0.0);
        assert!(!f.clipped);
    }

    #[test]
    fn a_calibration_run_completes_end_to_end() {
        let mut e = engine(vec![]);
        e.begin_calibration(CalibrationTarget::default());
        // A calibrator produces a 1 kHz tone; 0.05 amplitude is -23 dBFS.
        e.push_interleaved(&sine(1000.0, 0.05, 5.0, 1), 1);

        let status = e.calibration_status().expect("a run is in progress");
        assert!(status.is_ready(), "run was not ready: {status:?}");

        let cal = e.accept_calibration().expect("should accept");
        // A -23 dBFS measurement of a 94 dB source means full scale is 117 dB.
        assert!((cal.spl_from_dbfs(cal.measured_dbfs) - 94.0).abs() < 1e-9);
        assert!(e.frame().calibrated);
        assert!(e.calibration_status().is_none(), "run should have ended");
    }

    #[test]
    fn a_calibration_run_at_the_wrong_frequency_is_refused() {
        let mut e = engine(vec![]);
        e.begin_calibration(CalibrationTarget::default());
        e.push_interleaved(&sine(250.0, 0.05, 5.0, 1), 1);
        assert!(!e.calibration_status().unwrap().is_ready());
        assert!(e.accept_calibration().is_none());
    }

    #[test]
    fn empty_and_ragged_input_is_survivable() {
        let mut e = engine(vec![]);
        e.push_interleaved(&[], 2);
        e.push_interleaved(&[0.1, 0.2, 0.3], 0);
        // Three samples across two channels: one whole frame, one stray sample.
        e.push_interleaved(&[0.1, 0.2, 0.3], 2);
        let f = e.frame();
        assert!(f.spl.iter().all(|s| s.level.is_finite()));
        assert!(f.elapsed_seconds > 0.0);
    }

    #[test]
    fn changing_resolution_bumps_the_plan_revision() {
        let mut e = engine(vec![]);
        let before = e.frame().plan_revision;
        let mut cfg = e.config().clone();
        cfg.spectrum.fraction = Fraction::Twelfth;
        e.reconfigure(cfg, RATE);
        assert!(e.frame().plan_revision > before);
        assert_eq!(e.frame().bands_db.len(), e.plan().bands.len());
    }

    #[test]
    fn a_sample_rate_change_rebuilds_the_filters() {
        let mut e = engine(vec![]);
        e.push_interleaved(&sine(1000.0, 0.5, 1.0, 1), 1);
        let cfg = e.config().clone();
        e.reconfigure(cfg, 96000.0);
        assert_eq!(e.sample_rate(), 96000.0);
        // And it still measures correctly at the new rate.
        let frames = 96000;
        let buf: Vec<f32> = (0..frames)
            .map(|i| (2.0 * std::f64::consts::PI * 1000.0 * i as f64 / 96000.0).sin() as f32)
            .collect();
        e.push_interleaved(&buf, 1);
        assert!(e.frame().spl[2].level.abs() < 0.2);
    }
}
