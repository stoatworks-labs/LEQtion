//! Writing the measurement to disk, one row per history interval.
//!
//! A log is the copy somebody keeps. It outlives the window, gets attached to
//! an email, and is read months later by a person who was not in the room — so
//! everything needed to interpret it has to be *in the file*, not in the memory
//! of whoever ran it.
//!
//! Three consequences, and they are the whole design:
//!
//! * **The rows are the chart's own points.** A logger with its own timer would
//!   sample the same measurement on a different clock, and the CSV and the trace
//!   would disagree about the same second. Instead `Engine::push_interleaved`
//!   reports when a history interval completes and a row is written from
//!   `Engine::history_latest`, so the log *is* the chart.
//!
//! * **Every row says whether it is a sound pressure level.** A calibration can
//!   be applied or cleared mid-log, so this cannot live only in the header:
//!   `calibrated` is a column, and while it is false the numbers are full-scale
//!   levels. A column of dB with no unit is the one thing a measurement log must
//!   never be.
//!
//! * **Dropped audio is in the file.** If the analysis thread fell behind, time
//!   went missing from the measurement, and a log that closed the gap silently
//!   would be a fabrication. `dropped_frames` is cumulative, so any row where it
//!   increases marks a period that is short by an unknown amount.
//!
//! The format is CSV with `#` metadata lines above the header. Comment lines are
//! what pandas (`comment='#'`) and R (`comment.char='#'`) already expect, and
//! Excel shows them as ordinary rows rather than choking.

use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::PathBuf;

use leqtion_dsp::history::{HistoryPoint, SeriesInfo};
use serde::Serialize;

/// What the UI shows about logging, whether or not one is running.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogStatus {
    pub running: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Rows written so far. Zero for a whole interval after starting is normal,
    /// not a stall — the first row lands when the first interval completes.
    pub rows: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    pub interval_seconds: f64,
    /// Set when a write failed. Logging stops; the measurement does not.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub struct Logger {
    path: PathBuf,
    file: BufWriter<File>,
    /// Series ids in column order, fixed when the log opens.
    ///
    /// Fixed on purpose: adding an LEQ mid-log would otherwise widen the file
    /// halfway down, which every CSV reader in existence handles by failing or,
    /// worse, by silently shifting every value one column left. New series are
    /// ignored until the next log; a column that stops existing is written
    /// empty.
    columns: Vec<String>,
    rows: u64,
    started_at: String,
    interval_seconds: f64,
}

/// What the header records about the run — everything a reader months later
/// needs in order to know what they are looking at.
pub struct LogMeta<'a> {
    pub interval_seconds: f64,
    pub calibrated: bool,
    pub device: Option<&'a str>,
    pub sample_rate: f64,
    pub version: &'a str,
    pub started_at: String,
}

impl Logger {
    /// Open a log and write its header.
    pub fn create(path: PathBuf, series: &[SeriesInfo], meta: LogMeta<'_>) -> Result<Self, String> {
        let LogMeta {
            interval_seconds,
            calibrated,
            device,
            sample_rate,
            version,
            started_at,
        } = meta;
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(|e| format!("could not create {dir:?}: {e}"))?;
        }
        let file = File::create(&path).map_err(|e| format!("could not open {path:?}: {e}"))?;
        let mut file = BufWriter::new(file);

        let unit = if calibrated { "dB SPL" } else { "dBFS" };
        writeln!(file, "# LEQtion {version}").map_err(io)?;
        writeln!(file, "# started,{started_at}").map_err(io)?;
        writeln!(file, "# device,{}", device.unwrap_or("unknown")).map_err(io)?;
        writeln!(file, "# sample_rate_hz,{sample_rate:.0}").map_err(io)?;
        writeln!(file, "# interval_seconds,{interval_seconds}").map_err(io)?;
        writeln!(file, "# level_unit_at_start,{unit}").map_err(io)?;
        writeln!(
            file,
            "# note,levels are full-scale until calibrated; see the calibrated column"
        )
        .map_err(io)?;
        writeln!(
            file,
            "# note,min and mean and max are over each interval; mean is an energy mean"
        )
        .map_err(io)?;
        writeln!(
            file,
            "# note,any row where dropped_frames increases covers a period missing audio"
        )
        .map_err(io)?;

        let mut header = vec![
            "elapsed_seconds".to_string(),
            "calibrated".to_string(),
            "dropped_frames".to_string(),
        ];
        for s in series {
            header.push(format!("{}_min", s.label));
            header.push(format!("{}_mean", s.label));
            header.push(format!("{}_max", s.label));
        }
        writeln!(file, "{}", header.join(",")).map_err(io)?;
        file.flush().map_err(io)?;

        Ok(Logger {
            path,
            file,
            columns: series.iter().map(|s| s.id.clone()).collect(),
            rows: 0,
            started_at,
            interval_seconds,
        })
    }

    /// Write one interval.
    ///
    /// Flushed every row rather than on close: a log is most valuable exactly
    /// when the thing being measured went wrong, which is also when the app is
    /// most likely to be killed. A buffered tail is a lost tail.
    pub fn write(
        &mut self,
        latest: &[(SeriesInfo, HistoryPoint)],
        calibrated: bool,
        dropped_frames: u64,
    ) -> Result<(), String> {
        let Some(t) = latest.first().map(|(_, p)| p.t) else {
            return Ok(());
        };

        let mut row = vec![
            format!("{t:.3}"),
            if calibrated { "1".into() } else { "0".into() },
            dropped_frames.to_string(),
        ];
        for id in &self.columns {
            match latest.iter().find(|(info, _)| &info.id == id) {
                Some((_, p)) => {
                    row.push(format!("{:.2}", p.min));
                    row.push(format!("{:.2}", p.mean));
                    row.push(format!("{:.2}", p.max));
                }
                // A series that has gone away since the log opened. Empty, not
                // zero: zero is a level.
                None => row.extend(["".to_string(), "".to_string(), "".to_string()]),
            }
        }

        writeln!(self.file, "{}", row.join(",")).map_err(io)?;
        self.file.flush().map_err(io)?;
        self.rows += 1;
        Ok(())
    }

    pub fn status(&self) -> LogStatus {
        LogStatus {
            running: true,
            path: Some(self.path.display().to_string()),
            rows: self.rows,
            started_at: Some(self.started_at.clone()),
            interval_seconds: self.interval_seconds,
            error: None,
        }
    }

    /// Only the tests need this — everything else reads the path back out of
    /// [`Logger::status`], which is what the UI shows.
    #[cfg(test)]
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }
}

fn io(e: std::io::Error) -> String {
    format!("could not write the log: {e}")
}

/// A filename that sorts chronologically and needs no quoting.
pub fn default_filename(now: &str) -> String {
    let safe: String = now
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    format!("leqtion-{safe}.csv")
}

#[cfg(test)]
mod tests {
    use super::*;
    use leqtion_dsp::history::SeriesKind;
    use leqtion_dsp::spl::TimeWeighting;
    use leqtion_dsp::weighting::Weighting;

    fn series() -> Vec<SeriesInfo> {
        let kinds = vec![
            SeriesKind::Spl {
                weighting: Weighting::A,
                time_weighting: TimeWeighting::Fast,
            },
            SeriesKind::Leq { id: "one".into() },
        ];
        kinds
            .into_iter()
            .map(|k| SeriesInfo {
                id: k.id(),
                label: k.label(),
                kind: k,
            })
            .collect()
    }

    fn point(t: f64, v: f64) -> HistoryPoint {
        HistoryPoint {
            t,
            min: v - 1.0,
            mean: v,
            max: v + 1.0,
        }
    }

    fn open(dir: &std::path::Path, calibrated: bool) -> Logger {
        Logger::create(
            dir.join("log.csv"),
            &series(),
            LogMeta {
                interval_seconds: 1.0,
                calibrated,
                device: Some("Test Device"),
                sample_rate: 48000.0,
                version: "0.1.0",
                started_at: "2026-08-01T12:00:00Z".into(),
            },
        )
        .expect("the log opens")
    }

    #[test]
    fn the_header_names_every_series_and_its_three_statistics() {
        let dir = std::env::temp_dir().join("leqtion-log-header");
        let _ = fs::remove_dir_all(&dir);
        let logger = open(&dir, false);
        let text = fs::read_to_string(logger.path()).unwrap();

        let header = text
            .lines()
            .find(|l| !l.starts_with('#'))
            .expect("a header row");
        assert_eq!(
            header,
            "elapsed_seconds,calibrated,dropped_frames,\
             LAF_min,LAF_mean,LAF_max,LEQ one_min,LEQ one_mean,LEQ one_max"
        );
        assert!(text.contains("# level_unit_at_start,dBFS"));
    }

    /// The column that stops a log being unreadable a month later.
    #[test]
    fn a_calibration_applied_mid_log_shows_up_row_by_row() {
        let dir = std::env::temp_dir().join("leqtion-log-cal");
        let _ = fs::remove_dir_all(&dir);
        let mut logger = open(&dir, false);

        let latest: Vec<(SeriesInfo, HistoryPoint)> = series()
            .into_iter()
            .zip([point(1.0, -30.0), point(1.0, -31.0)])
            .collect();
        logger.write(&latest, false, 0).unwrap();
        logger.write(&latest, true, 0).unwrap();

        let text = fs::read_to_string(logger.path()).unwrap();
        let rows: Vec<&str> = text.lines().filter(|l| !l.starts_with('#')).collect();
        assert_eq!(rows[1].split(',').nth(1), Some("0"));
        assert_eq!(rows[2].split(',').nth(1), Some("1"));
    }

    /// A gap in the audio has to be visible in the file, not just on screen.
    #[test]
    fn dropped_frames_are_recorded_so_a_gap_is_findable() {
        let dir = std::env::temp_dir().join("leqtion-log-drops");
        let _ = fs::remove_dir_all(&dir);
        let mut logger = open(&dir, true);

        let latest: Vec<(SeriesInfo, HistoryPoint)> = series()
            .into_iter()
            .zip([point(1.0, 70.0), point(1.0, 69.0)])
            .collect();
        logger.write(&latest, true, 0).unwrap();
        logger.write(&latest, true, 4096).unwrap();

        let text = fs::read_to_string(logger.path()).unwrap();
        let rows: Vec<&str> = text.lines().filter(|l| !l.starts_with('#')).collect();
        assert_eq!(rows[2].split(',').nth(2), Some("4096"));
        assert_eq!(logger.status().rows, 2);
    }

    /// Widening a CSV halfway down is how a reader silently shifts every value
    /// one column left, so the columns are fixed when the log opens.
    #[test]
    fn a_series_added_after_the_log_opened_does_not_widen_it() {
        let dir = std::env::temp_dir().join("leqtion-log-width");
        let _ = fs::remove_dir_all(&dir);
        let mut logger = open(&dir, true);

        let mut latest: Vec<(SeriesInfo, HistoryPoint)> = series()
            .into_iter()
            .zip([point(1.0, 70.0), point(1.0, 69.0)])
            .collect();
        let extra = SeriesKind::Leq { id: "late".into() };
        latest.push((
            SeriesInfo {
                id: extra.id(),
                label: extra.label(),
                kind: extra,
            },
            point(1.0, 60.0),
        ));
        logger.write(&latest, true, 0).unwrap();

        let text = fs::read_to_string(logger.path()).unwrap();
        let rows: Vec<&str> = text.lines().filter(|l| !l.starts_with('#')).collect();
        assert_eq!(
            rows[0].split(',').count(),
            rows[1].split(',').count(),
            "the row is a different width from the header"
        );
    }

    /// A series that disappears leaves a hole, not a level.
    #[test]
    fn a_missing_series_is_written_empty_rather_than_zero() {
        let dir = std::env::temp_dir().join("leqtion-log-missing");
        let _ = fs::remove_dir_all(&dir);
        let mut logger = open(&dir, true);

        let latest: Vec<(SeriesInfo, HistoryPoint)> =
            vec![(series().remove(0), point(1.0, 70.0))];
        logger.write(&latest, true, 0).unwrap();

        let text = fs::read_to_string(logger.path()).unwrap();
        let row = text.lines().last().unwrap();
        assert!(row.ends_with(",,,"), "expected empty cells, got {row}");
    }

    #[test]
    fn the_default_filename_is_sortable_and_needs_no_quoting() {
        let name = default_filename("2026-08-01T12:34:56Z");
        assert_eq!(name, "leqtion-2026-08-01T12-34-56Z.csv");
        assert!(!name.contains(':') && !name.contains(' '));
    }
}
