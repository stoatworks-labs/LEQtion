import { useEffect, useRef, useState } from 'react';

import { formatElapsed, formatLevel, levelName } from '../lib/format';
import { api } from '../lib/ipc';
import { dbTicks, dbToUnit, fitCanvas } from '../lib/plot';
import { useStore } from '../state/store';
import type { Tile } from '../state/store';
import type { LeqPeriods } from '../types';

/**
 * One bar per period of a periodic LEQ — LAeq,15min the way a licence reads
 * it — with an optional limit line and the running total since reset.
 *
 * Each bar is the engine's LEQ of that period, integrated from the period's
 * own energy (`leqtion-dsp::leq`), never an average of the readings the chart
 * happened to see. Three kinds of bar are drawn differently on purpose:
 *
 * - **complete** — boundary to boundary, comparable with a limit;
 * - **short** — the period running when the measurement started, which began
 *   part-way through. Hatched, because a six-minute LAeq drawn as a quarter
 *   hour would be compared with a quarter-hour limit by whoever reads it;
 * - **in progress** — the period now, outlined, its value so far.
 *
 * The limit is a display setting. The engine knows nothing about it.
 */

interface Options {
  leqId?: string;
  /** dB, in the LEQ's own unit. Null for none. */
  limit?: number | null;
  showTotal?: boolean;
  top?: number;
  bottom?: number;
}

/** The period chart advances once a period; this only has to keep up with the bar in progress. */
const REDRAW_MS = 1000;
const PAD_LEFT = 34;
const PAD_RIGHT = 8;
const PAD_TOP = 14;
const PAD_BOTTOM = 18;
/** Narrowest bar slot, in CSS pixels. Sets how many periods are asked for. */
const MIN_SLOT = 8;

const INK = '#2f9ee0';
const OVER = '#e5484d';
const TOTAL = '#f0a33a';
/** The tile background, to clear a gutter label from under the limit's. */
const PANEL = '#141821';

export function PeriodsTile({ tile }: { tile: Tile }) {
  const canvas = useRef<HTMLCanvasElement>(null);
  const leqs = useStore((s) => s.config.leqs);
  const addLeq = useStore((s) => s.addLeq);
  const setTileOptions = useStore((s) => s.setTileOptions);
  const running = useStore((s) => s.status.running);
  const opts = tile.options as Options;

  const periodic = leqs.filter((l) => l.window.kind === 'periodic');
  const id = opts.leqId && periodic.some((l) => l.id === opts.leqId) ? opts.leqId : periodic[0]?.id;
  const [data, setData] = useState<LeqPeriods | null>(null);

  useEffect(() => {
    if (!id) return;
    let live = true;
    const tick = async () => {
      const width = canvas.current?.getBoundingClientRect().width ?? 600;
      const slots = Math.max(4, Math.floor((width - PAD_LEFT - PAD_RIGHT) / MIN_SLOT));
      try {
        const d = await api.leqPeriods(id, slots);
        if (live) setData(d);
      } catch {
        if (live) setData(null);
      }
    };
    void tick();
    const timer = setInterval(() => void tick(), REDRAW_MS);
    return () => {
      live = false;
      clearInterval(timer);
    };
  }, [id, running]);

  const calibrated = data?.calibrated ?? false;
  const top = opts.top ?? (calibrated ? 110 : 0);
  const bottom = opts.bottom ?? (calibrated ? 40 : -80);
  const limit = opts.limit ?? null;
  const showTotal = opts.showTotal ?? true;

  useEffect(() => {
    const c = canvas.current;
    if (!c) return;
    const { width, height } = fitCanvas(c);
    const ctx = c.getContext('2d');
    if (!ctx) return;
    ctx.clearRect(0, 0, width, height);

    const plotW = Math.max(1, width - PAD_LEFT - PAD_RIGHT);
    const plotH = Math.max(1, height - PAD_TOP - PAD_BOTTOM);
    const y = (db: number) => PAD_TOP + dbToUnit(db, top, bottom) * plotH;
    const baseline = PAD_TOP + plotH;

    ctx.font = '10px ui-monospace, SFMono-Regular, Menlo, monospace';
    ctx.lineWidth = 1;
    ctx.textBaseline = 'middle';
    for (const v of dbTicks(top, bottom, plotH)) {
      const py = Math.round(y(v)) + 0.5;
      ctx.strokeStyle = 'rgba(255,255,255,0.08)';
      ctx.beginPath();
      ctx.moveTo(PAD_LEFT, py);
      ctx.lineTo(width - PAD_RIGHT, py);
      ctx.stroke();
      ctx.fillStyle = 'rgba(255,255,255,0.42)';
      ctx.textAlign = 'right';
      ctx.fillText(String(v), PAD_LEFT - 6, py);
    }

    const periods = data?.periods ?? [];
    if (periods.length === 0) return;

    // Newest on the right; a slot per period, as wide as the tile allows.
    const slot = Math.max(MIN_SLOT, Math.min(64, plotW / periods.length));
    const shown = periods.slice(-Math.floor(plotW / slot));
    const left = width - PAD_RIGHT - shown.length * slot;
    const barW = Math.max(2, slot - Math.max(1, Math.round(slot * 0.18)));

    shown.forEach((p, i) => {
      const x0 = left + i * slot;
      const level = p.value;
      if (!(level > -199)) return;
      const py = Math.min(baseline, y(level));
      const over = limit != null && level > limit;
      const colour = over ? OVER : INK;
      if (p.current) {
        ctx.strokeStyle = colour;
        ctx.strokeRect(x0 + 0.5, py + 0.5, barW - 1, baseline - py - 1);
        ctx.globalAlpha = 0.25;
        ctx.fillStyle = colour;
        ctx.fillRect(x0, py, barW, baseline - py);
        ctx.globalAlpha = 1;
      } else if (!p.complete) {
        ctx.fillStyle = colour;
        ctx.globalAlpha = 0.35;
        ctx.fillRect(x0, py, barW, baseline - py);
        ctx.globalAlpha = 1;
        hatch(ctx, x0, py, barW, baseline - py, colour);
      } else {
        ctx.fillStyle = colour;
        ctx.fillRect(x0, py, barW, baseline - py);
      }

      if (slot >= 34) {
        ctx.fillStyle = 'rgba(255,255,255,0.75)';
        ctx.textAlign = 'center';
        ctx.fillText(formatLevel(level), x0 + barW / 2, Math.max(PAD_TOP - 6, py - 7));
      }
    });

    // Time labels, thinned so they never overlap.
    const every = Math.max(1, Math.ceil(46 / slot));
    ctx.fillStyle = 'rgba(255,255,255,0.42)';
    ctx.textAlign = 'center';
    shown.forEach((p, i) => {
      if ((shown.length - 1 - i) % every !== 0) return;
      const label = p.clock ?? formatElapsed(p.startSeconds);
      ctx.fillText(label, left + i * slot + barW / 2, height - PAD_BOTTOM / 2);
    });

    if (showTotal) {
      ctx.strokeStyle = TOTAL;
      ctx.fillStyle = TOTAL;
      ctx.lineWidth = 1.5;
      ctx.beginPath();
      let started = false;
      shown.forEach((p, i) => {
        if (!(p.total > -199)) return;
        const px = left + i * slot + barW;
        const py = y(p.total);
        if (started) ctx.lineTo(px, py);
        else ctx.moveTo(px, py);
        started = true;
      });
      ctx.stroke();
      ctx.lineWidth = 1;
    }

    // The limit is labelled in the axis gutter, where no bar or value label
    // can sit on top of it, and named in the header.
    if (limit != null) {
      const py = Math.round(y(limit)) + 0.5;
      ctx.strokeStyle = OVER;
      ctx.setLineDash([5, 4]);
      ctx.beginPath();
      ctx.moveTo(PAD_LEFT, py);
      ctx.lineTo(width - PAD_RIGHT, py);
      ctx.stroke();
      ctx.setLineDash([]);
      ctx.fillStyle = PANEL;
      ctx.fillRect(0, py - 7, PAD_LEFT - 2, 14);
      ctx.fillStyle = OVER;
      ctx.textAlign = 'right';
      ctx.fillText(String(limit), PAD_LEFT - 6, py);
    }

    ctx.strokeStyle = 'rgba(255,255,255,0.18)';
    ctx.beginPath();
    ctx.moveTo(PAD_LEFT + 0.5, PAD_TOP);
    ctx.lineTo(PAD_LEFT + 0.5, baseline + 0.5);
    ctx.lineTo(width - PAD_RIGHT, baseline + 0.5);
    ctx.stroke();
  }, [data, top, bottom, limit, showTotal]);

  if (!id) {
    return (
      <div className="tile-body">
        <div className="tile-empty">
          <p>
            No periodic LEQ to draw. A periodic LEQ is one figure per fixed period on the clock —
            LAeq,15min the way a licence usually means it.
          </p>
          <button
            type="button"
            onClick={() =>
              void addLeq({ label: '', weighting: 'a', window: { kind: 'periodic', seconds: 900 } }).then(
                (leqId) => setTileOptions(tile.id, { leqId }),
              )
            }
          >
            Add LAeq,15min periods
          </button>
        </div>
      </div>
    );
  }

  const spec = periodic.find((l) => l.id === id);
  const current = data?.periods[data.periods.length - 1];
  const finished = data?.periods.filter((p) => !p.current) ?? [];
  const latest = finished[finished.length - 1];

  return (
    <div className="tile-body chart">
      <div className="chart-head periods-head">
        <span className="chart-name">
          {data?.label ?? (spec ? spec.label || levelName(spec.weighting, spec.window) : id)}
        </span>
        <span className={calibrated ? 'chart-unit' : 'chart-unit uncal'}>
          {calibrated ? 'dB SPL' : 'dBFS'}
        </span>
        <span className="periods-now" title="The last finished period">
          last {formatLevel(latest?.value)}
          {latest && !latest.complete && ' (short)'}
        </span>
        <span className="periods-now" title="The period in progress, so far">
          now {formatLevel(current?.value)}
        </span>
        {showTotal && (
          <span className="periods-total" title="LEQ of everything since reset">
            total {formatLevel(current?.total)}
          </span>
        )}
        {limit != null && <span className="periods-limit">limit {limit}</span>}
      </div>
      <canvas ref={canvas} className="chart-canvas" />
      <p className="chart-note">
        Solid bars ran boundary to boundary. Hatched: started part-way, so not comparable with a
        limit for the full period. Outlined: in progress.
      </p>
    </div>
  );
}

function hatch(
  ctx: CanvasRenderingContext2D,
  x: number,
  y: number,
  w: number,
  h: number,
  colour: string,
) {
  ctx.save();
  ctx.beginPath();
  ctx.rect(x, y, w, h);
  ctx.clip();
  ctx.strokeStyle = colour;
  ctx.lineWidth = 1;
  ctx.beginPath();
  for (let d = -h; d < w; d += 5) {
    ctx.moveTo(x + d, y + h);
    ctx.lineTo(x + d + h, y);
  }
  ctx.stroke();
  ctx.restore();
}

export function PeriodsSettings({ tile }: { tile: Tile }) {
  const setTileOptions = useStore((s) => s.setTileOptions);
  const leqs = useStore((s) => s.config.leqs);
  const calibrated = useStore((s) => s.calibration != null);
  const opts = tile.options as Options;
  const periodic = leqs.filter((l) => l.window.kind === 'periodic');
  const id = opts.leqId ?? periodic[0]?.id;

  return (
    <>
      <label>
        Show
        <select value={id ?? ''} onChange={(e) => setTileOptions(tile.id, { leqId: e.target.value })}>
          {periodic.length === 0 && <option value="">no periodic LEQs defined</option>}
          {periodic.map((l) => (
            <option key={l.id} value={l.id}>
              {l.label || levelName(l.weighting, l.window)}
            </option>
          ))}
        </select>
      </label>
      <label>
        Limit
        <input
          type="number"
          step={1}
          placeholder="none"
          value={opts.limit ?? ''}
          onChange={(e) =>
            setTileOptions(tile.id, {
              limit: e.target.value === '' ? null : Number(e.target.value),
            })
          }
        />
      </label>
      <label className="check">
        <input
          type="checkbox"
          checked={opts.showTotal ?? true}
          onChange={(e) => setTileOptions(tile.id, { showTotal: e.target.checked })}
        />
        Running total since reset
      </label>
      <label>
        Top
        <input
          type="number"
          step={5}
          value={opts.top ?? (calibrated ? 110 : 0)}
          onChange={(e) => setTileOptions(tile.id, { top: Number(e.target.value) })}
        />
      </label>
      <label>
        Bottom
        <input
          type="number"
          step={5}
          value={opts.bottom ?? (calibrated ? 40 : -80)}
          onChange={(e) => setTileOptions(tile.id, { bottom: Number(e.target.value) })}
        />
      </label>
      <p className="hint">
        Periodic LEQs are added and edited in the toolbar&rsquo;s LEQ list. Periods sit on the clock
        — quarter hours start at :00, :15, :30 and :45.
      </p>
    </>
  );
}
