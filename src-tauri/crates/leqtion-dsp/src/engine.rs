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
}

impl Default for EngineConfig {
    fn default() -> Self {
        EngineConfig {
            spectrum: SpectrumConfig::default(),
            time_weighting: TimeWeighting::Fast,
            channel: ChannelSelect::default(),
            leqs: Vec::new(),
        }
    }
}

/// One weighted signal path: filter, detector, min/max and peak.
struct Path {
    weighting: Weighting,
    filter: WeightingFilter,
    detector: LevelDetector,
    minmax: MinMax,
    peak: PeakTracker,
    scratch: Vec<f32>,
    /// Mean square of the most recent block, for the LEQ accumulators.
    block_ms: f64,
}

impl Path {
    fn new(weighting: Weighting, time_weighting: TimeWeighting, sample_rate: f64) -> Self {
        Path {
            weighting,
            filter: WeightingFilter::new(weighting, sample_rate),
            detector: LevelDetector::new(time_weighting, sample_rate),
            minmax: MinMax::default(),
            peak: PeakTracker::default(),
            scratch: Vec::new(),
            block_ms: 0.0,
        }
    }

    fn push(&mut self, mono: &[f32]) {
        self.scratch.clear();
        self.scratch.extend_from_slice(mono);
        self.filter.process_block(&mut self.scratch);

        self.peak.push(&self.scratch);
        self.detector.push(&self.scratch);
        self.minmax.push(self.detector.mean_square());

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
}

impl Engine {
    pub fn new(config: EngineConfig, sample_rate: f64) -> Self {
        let spectrum = SpectrumAnalyser::new(config.spectrum, sample_rate);
        let paths = Weighting::ALL
            .iter()
            .map(|&w| Path::new(w, config.time_weighting, sample_rate))
            .collect();
        let leqs = config
            .leqs
            .iter()
            .cloned()
            .map(LeqAccumulator::new)
            .collect();

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

        if rate_changed || config.time_weighting != self.config.time_weighting {
            self.paths = Weighting::ALL
                .iter()
                .map(|&w| Path::new(w, config.time_weighting, sample_rate))
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

        self.sample_rate = sample_rate;
        self.config = config;
    }

    /// Feed interleaved input.
    pub fn push_interleaved(&mut self, samples: &[f32], channels: usize) {
        if channels == 0 || samples.is_empty() {
            return;
        }
        let frames = samples.len() / channels;
        if frames == 0 {
            return;
        }

        self.mono.clear();
        self.mono.reserve(frames);
        match self.config.channel {
            ChannelSelect::Channel { index } => {
                let c = index.min(channels - 1);
                for f in 0..frames {
                    self.mono.push(samples[f * channels + c]);
                }
            }
            ChannelSelect::Mix => {
                let inv = 1.0 / channels as f32;
                for f in 0..frames {
                    let mut sum = 0.0f32;
                    for c in 0..channels {
                        sum += samples[f * channels + c];
                    }
                    self.mono.push(sum * inv);
                }
            }
        }

        let seconds = frames as f64 / self.sample_rate;
        self.elapsed += seconds;

        self.input_peak.push(&self.mono);
        // Take the mono buffer out so the paths can be borrowed mutably
        // alongside it. It is put straight back; nothing else touches it.
        let mono = std::mem::take(&mut self.mono);
        self.spectrum.push(&mono);
        for p in &mut self.paths {
            p.push(&mono);
        }

        for acc in &mut self.leqs {
            let w = acc.spec().weighting;
            if let Some(p) = self.paths.iter().find(|p| p.weighting == w) {
                acc.push(p.block_ms, seconds);
            }
        }

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
    }

    /// Snapshot for one repaint.
    pub fn frame(&self) -> Frame {
        let calibrated = self.calibration.is_some();

        let spl = self
            .paths
            .iter()
            .map(|p| SplReading {
                weighting: p.weighting,
                level: self.to_level(p.detector.level_dbfs()),
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
        self.elapsed = 0.0;
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
