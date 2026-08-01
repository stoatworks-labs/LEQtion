import { useEffect, useRef, useState } from 'react';

import { api } from '../lib/ipc';
import { dbTicks, dbToUnit, fitCanvas } from '../lib/plot';
import { useStore } from '../state/store';
import type { Tile } from '../state/store';
import type { HistoryPoint, SeriesInfo } from '../types';

/**
 * Level over time: any series the engine is recording, as a line.
 *
 * Two things about this chart are deliberate and easy to undo by accident.
 *
 * **The band is not decoration.** Each point covers a whole interval, and the
 * shaded band is the min-to-max of what the level actually did inside it. A
 * chart drawing only the mean would look calmer than the measurement was, and
 * calmer is the one direction a level display must never be wrong in. The line
 * is the energy mean; the band is what it is hiding.
 *
 * **The points come from the engine, already bucketed.** Asking for more points
 * than there are pixels and thinning them here would drop peaks — so the width
 * is sent with the request and `history_view` does the reduction where the
 * numbers live. See `leqtion-dsp::history`.
 */

interface Options {
  seriesId?: string;
  spanSeconds?: number;
}

const SPANS = [
  { seconds: 60, label: '1 min' },
  { seconds: 300, label: '5 min' },
  { seconds: 900, label: '15 min' },
  { seconds: 3600, label: '1 hour' },
];

/** Refresh rate. The history advances once an interval; this only has to keep up. */
const REDRAW_MS = 500;

export function ChartTile({ tile }: { tile: Tile }) {
  const canvas = useRef<HTMLCanvasElement>(null);
  const calibrated = useStore((s) => s.status.running && s.calibration !== null);
  const opts = tile.options as Options;
  const seriesId = opts.seriesId ?? 'spl:a:f';
  const span = opts.spanSeconds ?? 300;

  const [points, setPoints] = useState<HistoryPoint[]>([]);
  const [series, setSeries] = useState<SeriesInfo[]>([]);

  useEffect(() => {
    let live = true;
    void api.historySeries().then((s) => live && setSeries(s));
    return () => {
      live = false;
    };
  }, []);

  useEffect(() => {
    let live = true;
    const tick = async () => {
      // The canvas width in device pixels is the point budget: one point per
      // column is the most a line can show, and asking for more would only move
      // the thinning into the browser.
      const width = canvas.current?.width ?? 600;
      try {
        const p = await api.historyView(seriesId, span, Math.max(2, Math.floor(width)));
        if (live) setPoints(p);
      } catch {
        if (live) setPoints([]);
      }
    };
    void tick();
    const timer = setInterval(() => void tick(), REDRAW_MS);
    return () => {
      live = false;
      clearInterval(timer);
    };
  }, [seriesId, span]);

  useEffect(() => {
    const c = canvas.current;
    if (!c) return;
    const { width, height } = fitCanvas(c);
    const ctx = c.getContext('2d');
    if (!ctx) return;

    ctx.clearRect(0, 0, width, height);

    // The scale follows the data rather than being fixed: an uncalibrated
    // measurement sits near -40 dBFS and a calibrated one near 80 dB SPL, and a
    // single hard-coded range would put one of them off the top.
    const values = points.flatMap((p) => [p.min, p.max]).filter((v) => v > -190);
    const top = values.length ? Math.ceil(Math.max(...values) / 10) * 10 + 5 : 0;
    const bottom = values.length ? Math.floor(Math.min(...values) / 10) * 10 - 5 : -100;

    // The same greys and blue the RTA draws with, written the same way, so the
    // two charts read as one instrument rather than two.
    const GRID = 'rgba(255,255,255,0.08)';
    const LABEL = 'rgba(255,255,255,0.42)';
    const INK = '#2f9ee0';

    ctx.strokeStyle = GRID;
    ctx.fillStyle = LABEL;
    ctx.lineWidth = 1;
    ctx.font = '10px ui-sans-serif, system-ui, sans-serif';
    for (const db of dbTicks(top, bottom, height)) {
      const y = Math.round(dbToUnit(db, top, bottom) * height) + 0.5;
      ctx.beginPath();
      ctx.moveTo(28, y);
      ctx.lineTo(width, y);
      ctx.stroke();
      // Only label a gridline with room for the whole number. A tick at the very
      // top or bottom of the canvas otherwise draws a half-height digit against
      // the tile edge, which reads as a rendering fault rather than as a scale.
      if (y > 10 && y < height - 3) ctx.fillText(String(db), 2, y + 3);
    }

    if (points.length < 2) {
      ctx.fillStyle = LABEL;
      ctx.fillText('waiting for the first interval…', 36, Math.round(height / 2));
      return;
    }

    const t0 = points[0].t;
    const t1 = points[points.length - 1].t;
    const dt = Math.max(1e-6, t1 - t0);
    const x = (t: number) => 28 + ((t - t0) / dt) * (width - 30);
    const y = (db: number) => dbToUnit(db, top, bottom) * height;

    // Min-to-max band first, so the mean line sits on top of it.
    ctx.fillStyle = INK;
    ctx.globalAlpha = 0.22;
    ctx.beginPath();
    ctx.moveTo(x(points[0].t), y(points[0].max));
    for (const p of points) ctx.lineTo(x(p.t), y(p.max));
    for (let i = points.length - 1; i >= 0; i--) {
      ctx.lineTo(x(points[i].t), y(points[i].min));
    }
    ctx.closePath();
    ctx.fill();
    ctx.globalAlpha = 1;

    ctx.strokeStyle = INK;
    ctx.lineWidth = 1.5;
    ctx.beginPath();
    points.forEach((p, i) => {
      const px = x(p.t);
      const py = y(p.mean);
      if (i === 0) ctx.moveTo(px, py);
      else ctx.lineTo(px, py);
    });
    ctx.stroke();
  }, [points]);

  const info = series.find((s) => s.id === seriesId);

  return (
    <div className="tile-body chart">
      <div className="chart-head">
        <span className="chart-name">{info?.label ?? seriesId}</span>
        <span className={calibrated ? 'chart-unit' : 'chart-unit uncal'}>
          {calibrated ? 'dB SPL' : 'dBFS'}
        </span>
        <span className="chart-span">{SPANS.find((s) => s.seconds === span)?.label ?? `${span}s`}</span>
      </div>
      <canvas ref={canvas} className="chart-canvas" />
      <p className="chart-note">
        Line is the energy mean per interval; the band is the min and max inside it.
      </p>
    </div>
  );
}

export function ChartSettings({ tile }: { tile: Tile }) {
  const setTileOptions = useStore((s) => s.setTileOptions);
  const setConfig = useStore((s) => s.setConfig);
  const history = useStore((s) => s.config.history);
  const opts = tile.options as Options;
  const [series, setSeries] = useState<SeriesInfo[]>([]);

  useEffect(() => {
    let live = true;
    void api.historySeries().then((s) => live && setSeries(s));
    return () => {
      live = false;
    };
  }, []);

  return (
    <>
      <label>
        Series
        <select
          value={opts.seriesId ?? 'spl:a:f'}
          onChange={(e) => setTileOptions(tile.id, { seriesId: e.target.value })}
        >
          {series.map((s) => (
            <option key={s.id} value={s.id}>
              {s.label}
            </option>
          ))}
        </select>
      </label>

      <label>
        Span
        <select
          value={String(opts.spanSeconds ?? 300)}
          onChange={(e) => setTileOptions(tile.id, { spanSeconds: Number(e.target.value) })}
        >
          {SPANS.map((s) => (
            <option key={s.seconds} value={String(s.seconds)}>
              {s.label}
            </option>
          ))}
        </select>
      </label>

      {/*
        * The interval belongs to the engine, not to this tile: it decides how
        * the history is recorded and therefore how often the log writes a row.
        * Two charts showing different spans still share one recording.
        */}
      <label>
        Interval
        <select
          value={String(history.intervalSeconds)}
          onChange={(e) =>
            void setConfig((c) => ({
              ...c,
              history: { ...c.history, intervalSeconds: Number(e.target.value) },
            }))
          }
        >
          {[0.1, 0.5, 1, 5, 10].map((s) => (
            <option key={s} value={String(s)}>
              {s < 1 ? `${s * 1000} ms` : `${s} s`}
            </option>
          ))}
        </select>
      </label>
      <p className="tile-note">
        The interval is shared with the log, so a row and a point are the same
        measurement. Changing it does not rewrite what is already recorded.
      </p>
    </>
  );
}
