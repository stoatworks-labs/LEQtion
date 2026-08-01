import { useRef } from 'react';

import { dbToUnit, dbTicks, fitCanvas, freqTicks, freqToUnit } from '../lib/plot';
import { useAnimationFrame, useFrameRef } from '../lib/useFrame';
import { useStore } from '../state/store';
import type { Tile } from '../state/store';

/**
 * The RTA: one bar per fractional-octave band.
 *
 * Drawn from the band plan the engine returned, never from a band table
 * computed here — that is what guarantees the bar under the "1k" label really
 * is the 1 kHz band, at every resolution.
 *
 * The region below `resolvedAboveHz` is shaded. Down there a 1/48-octave band is
 * narrower than the transform's bin spacing, so the value shown is interpolated
 * between bins rather than measured. It is still the best available answer, but
 * it is not detail the transform captured, and a display that draws it exactly
 * like the resolved bands is quietly lying about its own resolution.
 */

interface Options {
  top?: number;
  bottom?: number;
  showPeaks?: boolean;
}

const PAD_LEFT = 42;
const PAD_BOTTOM = 20;
const PAD_TOP = 8;
const PAD_RIGHT = 8;

export function RtaTile({ tile }: { tile: Tile }) {
  const canvas = useRef<HTMLCanvasElement>(null);
  const frameRef = useFrameRef();
  const plan = useStore((s) => s.plan);
  const calibrated = useStore((s) => s.calibration != null);

  const opts = tile.options as Options;
  // Uncalibrated levels are dBFS and sit below zero; calibrated ones are dB SPL
  // and sit well above it. Defaulting the axis per case saves everyone the first
  // action after every calibration.
  const top = opts.top ?? (calibrated ? 120 : 0);
  const bottom = opts.bottom ?? (calibrated ? 20 : -100);
  const showPeaks = opts.showPeaks ?? true;

  useAnimationFrame(() => {
    const el = canvas.current;
    if (!el || !plan) return;
    const ctx = el.getContext('2d');
    if (!ctx) return;

    const { width, height } = fitCanvas(el);
    const plotW = Math.max(1, width - PAD_LEFT - PAD_RIGHT);
    const plotH = Math.max(1, height - PAD_TOP - PAD_BOTTOM);

    ctx.clearRect(0, 0, width, height);

    const fMin = plan.bands[0]?.flo ?? 20;
    const fMax = plan.bands[plan.bands.length - 1]?.fhi ?? 20000;
    const x = (hz: number) => PAD_LEFT + freqToUnit(hz, fMin, fMax) * plotW;
    const y = (db: number) => PAD_TOP + dbToUnit(db, top, bottom) * plotH;

    // The unresolved region, shaded before anything is drawn over it.
    if (plan.resolvedAboveHz > fMin && Number.isFinite(plan.resolvedAboveHz)) {
      const edge = Math.min(plan.resolvedAboveHz, fMax);
      ctx.fillStyle = 'rgba(255,255,255,0.035)';
      ctx.fillRect(PAD_LEFT, PAD_TOP, x(edge) - PAD_LEFT, plotH);
    }

    // Grid.
    ctx.lineWidth = 1;
    ctx.font = '10px ui-monospace, SFMono-Regular, Menlo, monospace';
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

    for (const t of freqTicks(plotW, fMin, fMax)) {
      const px = Math.round(x(t.hz)) + 0.5;
      ctx.strokeStyle = t.major ? 'rgba(255,255,255,0.11)' : 'rgba(255,255,255,0.05)';
      ctx.beginPath();
      ctx.moveTo(px, PAD_TOP);
      ctx.lineTo(px, PAD_TOP + plotH);
      ctx.stroke();
      if (t.major) {
        ctx.fillStyle = 'rgba(255,255,255,0.42)';
        ctx.textAlign = 'center';
        ctx.fillText(t.label, px, height - PAD_BOTTOM / 2);
      }
    }

    const frame = frameRef.current;
    if (!frame || frame.bandsDb.length !== plan.bands.length) return;

    // Bars. Each spans its own band's edges, so the width carries the
    // resolution: a 1/3-octave bar is genuinely three times a 1/1-octave one.
    const baseline = PAD_TOP + plotH;
    for (let i = 0; i < plan.bands.length; i++) {
      const b = plan.bands[i];
      const x0 = x(b.flo);
      const x1 = x(b.fhi);
      const w = Math.max(1, x1 - x0 - (x1 - x0 > 3 ? 1 : 0));
      const level = frame.bandsDb[i];
      const py = y(level);
      if (py >= baseline) continue;
      ctx.fillStyle = b.fc < plan.resolvedAboveHz ? '#2b6f95' : '#2f9ee0';
      ctx.fillRect(x0, py, w, baseline - py);
    }

    if (showPeaks) {
      ctx.fillStyle = '#e8f4fd';
      for (let i = 0; i < plan.bands.length; i++) {
        const p = frame.peaksDb[i];
        if (!Number.isFinite(p)) continue;
        const b = plan.bands[i];
        const x0 = x(b.flo);
        const w = Math.max(1, x(b.fhi) - x0 - 1);
        ctx.fillRect(x0, Math.round(y(p)), w, 2);
      }
    }

    // Axis frame last, so bars never overdraw it.
    ctx.strokeStyle = 'rgba(255,255,255,0.18)';
    ctx.beginPath();
    ctx.moveTo(PAD_LEFT + 0.5, PAD_TOP);
    ctx.lineTo(PAD_LEFT + 0.5, baseline + 0.5);
    ctx.lineTo(width - PAD_RIGHT, baseline + 0.5);
    ctx.stroke();
  });

  return (
    <div className="tile-body">
      <canvas ref={canvas} className="fill" />
      {!plan && <p className="tile-empty">Waiting for a band plan…</p>}
    </div>
  );
}

export function RtaSettings({ tile }: { tile: Tile }) {
  const setTileOptions = useStore((s) => s.setTileOptions);
  const calibrated = useStore((s) => s.calibration != null);
  const opts = tile.options as Options;
  const top = opts.top ?? (calibrated ? 120 : 0);
  const bottom = opts.bottom ?? (calibrated ? 20 : -100);

  return (
    <>
      <label>
        Top
        <input
          type="number"
          value={top}
          step={5}
          onChange={(e) => setTileOptions(tile.id, { top: Number(e.target.value) })}
        />
      </label>
      <label>
        Bottom
        <input
          type="number"
          value={bottom}
          step={5}
          onChange={(e) => setTileOptions(tile.id, { bottom: Number(e.target.value) })}
        />
      </label>
      <label className="check">
        <input
          type="checkbox"
          checked={opts.showPeaks ?? true}
          onChange={(e) => setTileOptions(tile.id, { showPeaks: e.target.checked })}
        />
        Show held peaks
      </label>
    </>
  );
}
