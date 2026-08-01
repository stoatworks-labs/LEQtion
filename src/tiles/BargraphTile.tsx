import { useRef } from 'react';

import { dbTicks, dbToUnit, fitCanvas } from '../lib/plot';
import { useAnimationFrame, useFrameRef } from '../lib/useFrame';
import { useStore } from '../state/store';
import type { Tile } from '../state/store';
import { WEIGHTING_LABEL, type Weighting } from '../types';

/**
 * Full-height level meter: a bar for the time-weighted level, a line for the
 * held maximum, and a separate strip for the input's own peak.
 *
 * The peak strip is always in dBFS, even when everything else is in dB SPL, and
 * that is deliberate. Peak is a headroom figure — it answers "is the converter
 * about to clip", which is a question about the electrical signal and has no
 * meaningful answer in sound pressure. Showing it as an SPL would invite someone
 * to read 118 dB and think they had headroom when the input was already at 0 dBFS.
 */

interface Options {
  weighting?: Weighting;
  top?: number;
  bottom?: number;
}

const PAD_TOP = 10;
const PAD_BOTTOM = 18;
const LABEL_W = 34;
const PEAK_W = 12;
const GAP = 8;

export function BargraphTile({ tile }: { tile: Tile }) {
  const canvas = useRef<HTMLCanvasElement>(null);
  const frameRef = useFrameRef();
  const calibrated = useStore((s) => s.calibration != null);

  const opts = tile.options as Options;
  const weighting = opts.weighting ?? 'a';
  const top = opts.top ?? (calibrated ? 120 : 0);
  const bottom = opts.bottom ?? (calibrated ? 20 : -80);

  useAnimationFrame(() => {
    const el = canvas.current;
    if (!el) return;
    const ctx = el.getContext('2d');
    if (!ctx) return;

    const { width, height } = fitCanvas(el);
    const plotH = Math.max(1, height - PAD_TOP - PAD_BOTTOM);
    const barX = LABEL_W;
    const barW = Math.max(6, width - LABEL_W - PEAK_W - GAP - 4);
    const peakX = width - PEAK_W - 2;

    ctx.clearRect(0, 0, width, height);
    const y = (db: number) => PAD_TOP + dbToUnit(db, top, bottom) * plotH;

    ctx.font = '10px ui-monospace, SFMono-Regular, Menlo, monospace';
    ctx.textBaseline = 'middle';
    ctx.textAlign = 'right';
    for (const v of dbTicks(top, bottom, plotH)) {
      const py = Math.round(y(v)) + 0.5;
      ctx.strokeStyle = 'rgba(255,255,255,0.08)';
      ctx.beginPath();
      ctx.moveTo(barX, py);
      ctx.lineTo(width - 2, py);
      ctx.stroke();
      ctx.fillStyle = 'rgba(255,255,255,0.42)';
      ctx.fillText(String(v), LABEL_W - 6, py);
    }

    // Troughs.
    ctx.fillStyle = 'rgba(255,255,255,0.05)';
    ctx.fillRect(barX, PAD_TOP, barW, plotH);
    ctx.fillRect(peakX, PAD_TOP, PEAK_W, plotH);

    const frame = frameRef.current;
    const baseline = PAD_TOP + plotH;
    if (frame) {
      const spl = frame.spl.find((s) => s.weighting === weighting);
      if (spl) {
        const py = y(spl.level);
        ctx.fillStyle = '#2f9ee0';
        ctx.fillRect(barX, py, barW, baseline - py);

        // Held maximum.
        if (Number.isFinite(spl.max) && spl.max > bottom) {
          ctx.fillStyle = '#e8f4fd';
          ctx.fillRect(barX, Math.round(y(spl.max)) - 1, barW, 2);
        }
      }

      // Input peak, always dBFS. Its own scale: 0 dBFS at the top, -60 at the
      // bottom, because headroom is the only thing this strip is for.
      const peakU = Math.min(1, Math.max(0, -frame.inputPeakDbfs / 60));
      const peakY = PAD_TOP + peakU * plotH;
      ctx.fillStyle = frame.clipped ? '#e5484d' : frame.inputPeakDbfs > -6 ? '#f5a524' : '#3aa675';
      ctx.fillRect(peakX, peakY, PEAK_W, baseline - peakY);

      ctx.textAlign = 'center';
      ctx.fillStyle = frame.clipped ? '#e5484d' : 'rgba(255,255,255,0.5)';
      ctx.fillText(frame.clipped ? 'CLIP' : 'pk', peakX + PEAK_W / 2, height - PAD_BOTTOM / 2);
    }

    ctx.textAlign = 'center';
    ctx.fillStyle = 'rgba(255,255,255,0.55)';
    ctx.fillText(`L${WEIGHTING_LABEL[weighting]}`, barX + barW / 2, height - PAD_BOTTOM / 2);
  });

  return (
    <div className="tile-body">
      <canvas ref={canvas} className="fill" />
    </div>
  );
}

export function BargraphSettings({ tile }: { tile: Tile }) {
  const setTileOptions = useStore((s) => s.setTileOptions);
  const calibrated = useStore((s) => s.calibration != null);
  const opts = tile.options as Options;

  return (
    <>
      <label>
        Weighting
        <select
          value={opts.weighting ?? 'a'}
          onChange={(e) => setTileOptions(tile.id, { weighting: e.target.value })}
        >
          <option value="a">A</option>
          <option value="c">C</option>
          <option value="z">Z</option>
        </select>
      </label>
      <label>
        Top
        <input
          type="number"
          step={5}
          value={opts.top ?? (calibrated ? 120 : 0)}
          onChange={(e) => setTileOptions(tile.id, { top: Number(e.target.value) })}
        />
      </label>
      <label>
        Bottom
        <input
          type="number"
          step={5}
          value={opts.bottom ?? (calibrated ? 20 : -80)}
          onChange={(e) => setTileOptions(tile.id, { bottom: Number(e.target.value) })}
        />
      </label>
    </>
  );
}
