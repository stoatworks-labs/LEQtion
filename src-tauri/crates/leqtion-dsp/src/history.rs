//! Level history: the same numbers the meter shows, kept over time.
//!
//! A meter answers "how loud is it now". A history answers "how loud has it
//! been", which is the question a noise limit, a complaint or a licence
//! condition is actually about — and it is not answerable by remembering what
//! was on screen, because the screen only ever showed one instant of it.
//!
//! ## Why this lives in the engine
//!
//! For the same reason the LEQs do: it keeps recording whether or not a tile is
//! showing it. A history that only accumulated while a chart was open would go
//! blank the moment someone closed the tile, and — worse — a *log* built on it
//! would have holes in it that nothing on screen explained. Closing a window is
//! not an instruction to stop measuring.
//!
//! ## Every point is an interval, not a sample
//!
//! Recording the instantaneous value once a second and drawing a line through
//! the dots would alias: a 100 ms transient either lands on a tick or does not
//! exist, and the same measurement replayed would draw a different line. So a
//! point covers the whole interval it spans and carries three numbers —
//! `min`, `mean` and `max` — which is also exactly what a sound level meter
//! writes into a log: Lmin, Leq and Lmax per period.
//!
//! `mean` is an **energy** mean, computed on mean squares and converted back at
//! the end. A mean of decibels is not a level; see the crate docs.
//!
//! ## Downsampling is done here, not in the UI
//!
//! A chart 900 pixels wide showing an hour at 1 s per point has 4000 points to
//! draw. Picking every fourth one would drop three quarters of the peaks and
//! quietly flatten the trace — the one thing a level history must not do. So
//! [`History::view`] buckets instead, and each bucket keeps the min of the mins
//! and the max of the maxes. The line can only ever get *more* honest as it is
//! zoomed out, never less.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

use crate::spl::{TimeWeighting, SILENCE_DBFS};
use crate::weighting::Weighting;

/// Default seconds between recorded points.
pub const DEFAULT_INTERVAL_SECONDS: f64 = 1.0;

/// Default span kept in memory.
pub const DEFAULT_SPAN_SECONDS: f64 = 60.0 * 60.0;

/// Intervals offered in the UI. Anything shorter than 100 ms records faster
/// than a Slow detector can respond, which is a lot of points saying the same
/// thing; anything longer than a minute is a log, not a chart.
pub const OFFERED_INTERVALS: [f64; 5] = [0.1, 0.5, 1.0, 5.0, 10.0];

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryConfig {
    pub interval_seconds: f64,
    pub span_seconds: f64,
}

impl Default for HistoryConfig {
    fn default() -> Self {
        HistoryConfig {
            interval_seconds: DEFAULT_INTERVAL_SECONDS,
            span_seconds: DEFAULT_SPAN_SECONDS,
        }
    }
}

impl HistoryConfig {
    /// Points kept per series. At least two, so a chart always has a line
    /// rather than a dot.
    pub fn capacity(&self) -> usize {
        let interval = self.interval_seconds.max(0.01);
        ((self.span_seconds / interval).ceil() as usize).clamp(2, 1_000_000)
    }
}

/// What a series is measuring.
///
/// Carried as data rather than baked into the id string so the UI can label a
/// series properly — "LAF" is not a name anyone should have to parse out of
/// `spl:a:fast`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SeriesKind {
    /// Time-weighted sound level, one per frequency and time weighting.
    Spl {
        weighting: Weighting,
        time_weighting: TimeWeighting,
    },
    /// Sample peak, per frequency weighting. Only `max` means much on a peak
    /// series — see [`HistoryPoint`].
    Peak { weighting: Weighting },
    /// One of the user's LEQs, by its own id.
    ///
    /// Renamed on the wire. [`SeriesInfo`] flattens this enum beside its own
    /// `id`, and a variant field also called `id` silently overwrites it — the
    /// series then arrives in the UI identified by the LEQ's id rather than the
    /// series id, so every lookup by id misses and the chart falls back to
    /// showing a raw key as its title.
    Leq {
        #[serde(rename = "leqId")]
        id: String,
    },
}

impl SeriesKind {
    /// Stable identifier, used as the ring's key across reconfigurations.
    pub fn id(&self) -> String {
        match self {
            SeriesKind::Spl {
                weighting,
                time_weighting,
            } => format!(
                "spl:{}:{}",
                weighting.label().to_lowercase(),
                time_weighting.label().to_lowercase()
            ),
            SeriesKind::Peak { weighting } => {
                format!("peak:{}", weighting.label().to_lowercase())
            }
            SeriesKind::Leq { id } => format!("leq:{id}"),
        }
    }

    /// What a meter would call this, e.g. "LAF", "LCpeak".
    pub fn label(&self) -> String {
        match self {
            SeriesKind::Spl {
                weighting,
                time_weighting,
            } => format!("L{}{}", weighting.label(), time_weighting.label()),
            SeriesKind::Peak { weighting } => format!("L{}peak", weighting.label()),
            SeriesKind::Leq { id } => format!("LEQ {id}"),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeriesInfo {
    pub id: String,
    pub label: String,
    #[serde(flatten)]
    pub kind: SeriesKind,
}

/// One interval's worth of one series.
///
/// `min` and `max` are the extremes the level actually reached inside the
/// interval, not the ends of it. On a [`SeriesKind::Peak`] series all three are
/// statistics of the per-block sample peak, of which only `max` is a peak in
/// the sense anyone means — the others are recorded for consistency and are not
/// worth plotting.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryPoint {
    /// Seconds since the measurement started, at the **end** of the interval.
    pub t: f64,
    pub min: f64,
    pub mean: f64,
    pub max: f64,
}

/// Accumulates one interval, in the energy domain.
#[derive(Debug, Clone, Copy)]
struct Bucket {
    min: f64,
    max: f64,
    /// Time-weighted sum of mean squares, for the energy mean.
    energy: f64,
    seconds: f64,
}

impl Default for Bucket {
    fn default() -> Self {
        Bucket {
            min: f64::INFINITY,
            max: f64::NEG_INFINITY,
            energy: 0.0,
            seconds: 0.0,
        }
    }
}

impl Bucket {
    fn push(&mut self, db: f64, dt: f64) {
        if dt <= 0.0 {
            return;
        }
        self.min = self.min.min(db);
        self.max = self.max.max(db);
        // Silence is a floor, not a real level: folding 10^(-200/10) into the
        // energy sum is harmless arithmetically but it is also the value that
        // makes a long quiet stretch read as exactly the floor rather than as
        // whatever it really was, so it is carried as-is.
        self.energy += 10f64.powf(db / 10.0) * dt;
        self.seconds += dt;
    }

    fn finish(&self, t: f64) -> HistoryPoint {
        if self.seconds <= 0.0 {
            return HistoryPoint {
                t,
                min: SILENCE_DBFS,
                mean: SILENCE_DBFS,
                max: SILENCE_DBFS,
            };
        }
        let mean = 10.0 * (self.energy / self.seconds).log10();
        HistoryPoint {
            t,
            min: self.min.max(SILENCE_DBFS),
            mean: mean.max(SILENCE_DBFS),
            max: self.max.max(SILENCE_DBFS),
        }
    }
}

struct Series {
    info: SeriesInfo,
    points: VecDeque<HistoryPoint>,
    bucket: Bucket,
}

/// The recorder.
///
/// Fed one value per series per audio block; emits one point per series per
/// interval. Time comes from the caller's block durations, not from a clock, so
/// the history advances at exactly the rate audio actually arrived — a stalled
/// input leaves a gap rather than a flat line drawn over missing seconds.
pub struct History {
    config: HistoryConfig,
    series: Vec<Series>,
    /// Seconds accumulated into the current, unfinished interval.
    filling: f64,
    /// Seconds since the last reset, at the end of the last completed interval.
    elapsed: f64,
}

impl History {
    pub fn new(config: HistoryConfig, kinds: Vec<SeriesKind>) -> Self {
        let mut history = History {
            config,
            series: Vec::new(),
            filling: 0.0,
            elapsed: 0.0,
        };
        history.set_series(kinds);
        history
    }

    pub fn config(&self) -> HistoryConfig {
        self.config
    }

    /// Change the interval or span.
    ///
    /// Points already recorded are kept but **not** re-bucketed: they were
    /// measured over a different interval and rewriting them would invent
    /// numbers. Shrinking the span drops the oldest.
    pub fn reconfigure(&mut self, config: HistoryConfig) {
        self.config = config;
        let capacity = config.capacity();
        for s in &mut self.series {
            while s.points.len() > capacity {
                s.points.pop_front();
            }
        }
        self.filling = 0.0;
        for s in &mut self.series {
            s.bucket = Bucket::default();
        }
    }

    /// Replace the series list, keeping the points of any that survive.
    ///
    /// Matched by id, so adding a fourth LEQ does not throw away the history of
    /// the first three — which is the whole reason this is not just a rebuild.
    pub fn set_series(&mut self, kinds: Vec<SeriesKind>) {
        let capacity = self.config.capacity();
        let mut existing: Vec<Series> = std::mem::take(&mut self.series);

        self.series = kinds
            .into_iter()
            .map(|kind| {
                let id = kind.id();
                let found = existing.iter().position(|s| s.info.id == id);
                match found {
                    Some(i) => {
                        let mut old = existing.remove(i);
                        old.info.label = kind.label();
                        old.info.kind = kind;
                        while old.points.len() > capacity {
                            old.points.pop_front();
                        }
                        old
                    }
                    None => Series {
                        info: SeriesInfo {
                            id,
                            label: kind.label(),
                            kind,
                        },
                        points: VecDeque::with_capacity(capacity.min(4096)),
                        bucket: Bucket::default(),
                    },
                }
            })
            .collect();
    }

    pub fn series(&self) -> Vec<SeriesInfo> {
        self.series.iter().map(|s| s.info.clone()).collect()
    }

    pub fn len(&self) -> usize {
        self.series.first().map(|s| s.points.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Seconds of history recorded since the last reset.
    pub fn elapsed(&self) -> f64 {
        self.elapsed + self.filling
    }

    /// Feed one block. `values` is in the order [`History::series`] returns.
    ///
    /// Returns true when the block completed an interval, which is the signal a
    /// logger writes a row on — so the log and the chart are the same points
    /// rather than two samplings of the same measurement that disagree.
    pub fn push(&mut self, dt: f64, values: &[f64]) -> bool {
        if dt <= 0.0 || values.len() != self.series.len() {
            return false;
        }

        let interval = self.config.interval_seconds.max(0.01);
        let mut remaining = dt;
        let mut completed = false;

        // A block longer than the interval fills several at once, so this
        // loops rather than assuming one block is at most one point. At a
        // 10 ms interval and a 4096-sample block that is nine points from one
        // push; dropping eight of them would silently thin the history under
        // exactly the settings someone chose for detail.
        while remaining > 0.0 {
            let room = interval - self.filling;
            let take = remaining.min(room);

            for (s, &v) in self.series.iter_mut().zip(values) {
                s.bucket.push(v, take);
            }
            self.filling += take;
            remaining -= take;

            if self.filling + 1e-12 >= interval {
                self.elapsed += interval;
                let capacity = self.config.capacity();
                for s in &mut self.series {
                    let point = s.bucket.finish(self.elapsed);
                    s.bucket = Bucket::default();
                    if s.points.len() >= capacity {
                        s.points.pop_front();
                    }
                    s.points.push_back(point);
                }
                self.filling = 0.0;
                completed = true;
            }
        }

        completed
    }

    /// The most recently completed point for one series.
    pub fn last(&self, id: &str) -> Option<HistoryPoint> {
        self.series
            .iter()
            .find(|s| s.info.id == id)
            .and_then(|s| s.points.back().copied())
    }

    /// The last `seconds` of one series, bucketed down to at most `max_points`.
    ///
    /// Bucketing keeps the min of the mins and the max of the maxes, so zooming
    /// out never hides a peak — the trace gets coarser in time and stays honest
    /// in level. The mean is re-averaged in the energy domain, which is the only
    /// way it stays a level.
    pub fn view(&self, id: &str, seconds: f64, max_points: usize) -> Vec<HistoryPoint> {
        let Some(s) = self.series.iter().find(|s| s.info.id == id) else {
            return Vec::new();
        };
        let max_points = max_points.max(1);

        let cutoff = self.elapsed - seconds.max(0.0);
        let wanted: Vec<&HistoryPoint> =
            s.points.iter().filter(|p| p.t > cutoff).collect();
        if wanted.len() <= max_points {
            return wanted.into_iter().copied().collect();
        }

        // Ceil, so the last bucket is never left holding a single point that
        // then reads as a spike of its own making.
        let per = wanted.len().div_ceil(max_points);
        wanted
            .chunks(per)
            .map(|chunk| {
                let mut min = f64::INFINITY;
                let mut max = f64::NEG_INFINITY;
                let mut energy = 0.0;
                for p in chunk {
                    min = min.min(p.min);
                    max = max.max(p.max);
                    energy += 10f64.powf(p.mean / 10.0);
                }
                HistoryPoint {
                    t: chunk.last().unwrap().t,
                    min,
                    mean: (10.0 * (energy / chunk.len() as f64).log10()).max(SILENCE_DBFS),
                    max,
                }
            })
            .collect()
    }

    /// Drop everything recorded. Called when the measurement is reset.
    pub fn reset(&mut self) {
        for s in &mut self.series {
            s.points.clear();
            s.bucket = Bucket::default();
        }
        self.filling = 0.0;
        self.elapsed = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spl_series() -> Vec<SeriesKind> {
        vec![SeriesKind::Spl {
            weighting: Weighting::A,
            time_weighting: TimeWeighting::Fast,
        }]
    }

    #[test]
    fn ids_and_labels_are_what_a_meter_would_call_them() {
        let spl = SeriesKind::Spl {
            weighting: Weighting::A,
            time_weighting: TimeWeighting::Fast,
        };
        assert_eq!(spl.id(), "spl:a:f");
        assert_eq!(spl.label(), "LAF");

        let peak = SeriesKind::Peak {
            weighting: Weighting::C,
        };
        assert_eq!(peak.id(), "peak:c");
        assert_eq!(peak.label(), "LCpeak");
    }

    #[test]
    fn a_point_is_emitted_once_per_interval() {
        let mut h = History::new(
            HistoryConfig {
                interval_seconds: 1.0,
                span_seconds: 60.0,
            },
            spl_series(),
        );

        // Ten blocks of 0.1s is exactly one interval, and exactly one point.
        for _ in 0..10 {
            h.push(0.1, &[-20.0]);
        }
        assert_eq!(h.len(), 1);
        for _ in 0..10 {
            h.push(0.1, &[-20.0]);
        }
        assert_eq!(h.len(), 2);
    }

    /// The property the whole module exists for: a transient inside an interval
    /// survives into the point, rather than being missed between samples.
    #[test]
    fn a_peak_between_ticks_is_not_lost() {
        let mut h = History::new(
            HistoryConfig {
                interval_seconds: 1.0,
                span_seconds: 60.0,
            },
            spl_series(),
        );

        for i in 0..10 {
            h.push(0.1, &[if i == 3 { -6.0 } else { -60.0 }]);
        }

        let p = h.last("spl:a:f").expect("one point");
        assert_eq!(p.max, -6.0, "the transient must reach max");
        assert_eq!(p.min, -60.0);
        // Energy mean: one tenth of the interval at -6 dB dominates.
        assert!(p.mean > -20.0 && p.mean < -15.0, "mean was {}", p.mean);
    }

    /// A mean of decibels would give -54.6 here; an energy mean gives -47.0.
    #[test]
    fn the_mean_is_an_energy_mean_not_an_average_of_decibels() {
        let mut h = History::new(
            HistoryConfig {
                interval_seconds: 1.0,
                span_seconds: 60.0,
            },
            spl_series(),
        );
        h.push(0.5, &[-50.0]);
        h.push(0.5, &[-60.0]);

        let p = h.last("spl:a:f").expect("one point");
        let decibel_mean = -55.0;
        assert!(
            (p.mean - -52.6).abs() < 0.1,
            "energy mean was {}, expected about -52.6",
            p.mean
        );
        assert!((p.mean - decibel_mean).abs() > 2.0);
    }

    #[test]
    fn a_block_longer_than_the_interval_fills_every_point_it_covers() {
        let mut h = History::new(
            HistoryConfig {
                interval_seconds: 0.1,
                span_seconds: 60.0,
            },
            spl_series(),
        );
        h.push(1.0, &[-30.0]);
        assert_eq!(h.len(), 10, "one second at 0.1s is ten points");
    }

    #[test]
    fn the_span_bounds_what_is_kept() {
        let mut h = History::new(
            HistoryConfig {
                interval_seconds: 1.0,
                span_seconds: 5.0,
            },
            spl_series(),
        );
        for i in 0..20 {
            h.push(1.0, &[-i as f64]);
        }
        assert_eq!(h.len(), 5);
        assert_eq!(h.last("spl:a:f").unwrap().t, 20.0);
    }

    /// Zooming out must not hide a peak — the failure this bucketing exists to
    /// prevent is a trace that gets flatter the further back you look.
    #[test]
    fn downsampling_keeps_the_extremes() {
        let mut h = History::new(
            HistoryConfig {
                interval_seconds: 1.0,
                span_seconds: 1000.0,
            },
            spl_series(),
        );
        for i in 0..100 {
            h.push(1.0, &[if i == 57 { 0.0 } else { -60.0 }]);
        }

        let view = h.view("spl:a:f", 1000.0, 10);
        assert_eq!(view.len(), 10);
        assert!(
            view.iter().any(|p| p.max == 0.0),
            "the spike was lost when the view was bucketed"
        );
    }

    #[test]
    fn adding_a_series_keeps_the_history_of_the_others() {
        let mut h = History::new(HistoryConfig::default(), spl_series());
        for _ in 0..5 {
            h.push(1.0, &[-30.0]);
        }
        assert_eq!(h.len(), 5);

        let mut kinds = spl_series();
        kinds.push(SeriesKind::Leq { id: "new".into() });
        h.set_series(kinds);

        assert_eq!(h.series().len(), 2);
        assert_eq!(
            h.view("spl:a:f", 1000.0, 100).len(),
            5,
            "the existing series lost its points"
        );
        assert!(h.view("leq:new", 1000.0, 100).is_empty());
    }

    /// The flatten collision: a LEQ series must keep its own id on the wire.
    #[test]
    fn a_leq_series_serialises_with_the_series_id_not_the_leq_id() {
        let kind = SeriesKind::Leq { id: "leq-a".into() };
        let info = SeriesInfo {
            id: kind.id(),
            label: kind.label(),
            kind,
        };
        let json = serde_json::to_value(&info).expect("serialises");
        assert_eq!(json["id"], "leq:leq-a", "{json}");
        assert_eq!(json["leqId"], "leq-a");
    }

    /// `rename_all` renames variants, not the fields inside them. `types.ts`
    /// declares `timeWeighting`, so a variant field left as `time_weighting`
    /// arrives undefined and the series loses its time weighting silently.
    #[test]
    fn a_spl_series_serialises_its_variant_fields_in_camel_case() {
        let kind = SeriesKind::Spl {
            weighting: Weighting::A,
            time_weighting: TimeWeighting::Fast,
        };
        let info = SeriesInfo {
            id: kind.id(),
            label: kind.label(),
            kind,
        };
        let json = serde_json::to_value(&info).expect("serialises");
        assert_eq!(json["timeWeighting"], "fast", "{json}");
        assert!(json.get("time_weighting").is_none(), "{json}");
    }

    #[test]
    fn a_mismatched_value_count_is_ignored_rather_than_recorded_wrong() {
        let mut h = History::new(HistoryConfig::default(), spl_series());
        assert!(!h.push(1.0, &[-30.0, -40.0]));
        assert_eq!(h.len(), 0);
    }

    #[test]
    fn reset_clears_everything_including_the_part_interval() {
        let mut h = History::new(HistoryConfig::default(), spl_series());
        h.push(0.5, &[-30.0]);
        h.reset();
        assert_eq!(h.len(), 0);
        assert_eq!(h.elapsed(), 0.0);
    }
}
