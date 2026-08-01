import { useEffect, useRef } from 'react';

import { fitCanvas, freqToUnit, heatRgba } from '../lib/plot';
import { useAnimationFrame, useFrameRef } from '../lib/useFrame';
import { useStore } from '../state/store';
import type { Tile } from '../state/store';

/**
 * Scrolling spectrograph: one column per update, time running right to left.
 *
 * ## How the scroll works
 *
 * The whole image is kept in an offscreen canvas one pixel column per update
 * wide. Each new column is drawn at the right-hand end and the read window
 * moves, rather than the image being blitted left every frame. Copying a canvas
 * onto itself every frame is the obvious approach and it degrades badly: it is a
 * full-surface read-modify-write at 30 Hz, and on a scaled display it also
 * resamples, so the picture softens a little more with every column until it is
 * a smear.
 *
 * Bands are drawn at their own edges on the same log axis as the RTA, so the two
 * tiles line up vertically when stacked — which is the main reason to have both.
 */

interface Options {
  top?: number;
  bottom?: number;
  /** Seconds of history across the full width. */
  span?: number;
}

const PAD_LEFT = 42;

export function SpectrographTile({ tile }: { tile: Tile }) {
  const canvas = useRef<HTMLCanvasElement>(null);
  const history = useRef<HTMLCanvasElement | null>(null);
  const writeCol = useRef(0);
  const lastPaint = useRef(0);
  const frameRef = useFrameRef();
  const plan = useStore((s) => s.plan);
  const calibrated = useStore((s) => s.calibration != null);

  const opts = tile.options as Options;
  const top = opts.top ?? (calibrated ? 110 : -10);
  const bottom = opts.bottom ?? (calibrated ? 30 : -90);
  const span = opts.span ?? 30;

  // A resolution change makes every stored column meaningless — the rows no
  // longer mean the same frequencies — so the history is thrown away rather
  // than left to show a discontinuity that looks like a real event.
  useEffect(() => {
    history.current = null;
    writeCol.current = 0;
  }, [plan?.fraction, plan?.sampleRate, top, bottom]);

  useAnimationFrame(() => {
    const el = canvas.current;
    if (!el || !plan) return;
    const ctx = el.getContext('2d');
    if (!ctx) return;

    const { width, height, changed } = fitCanvas(el);
    const plotW = Math.max(1, Math.floor(width - PAD_LEFT));
    const plotH = Math.max(1, Math.floor(height));

    if (!history.current || changed || history.current.width !== plotW || history.current.height !== plotH) {
      const c = document.createElement('canvas');
      c.width = plotW;
      c.height = plotH;
      const hctx = c.getContext('2d');
      if (hctx) {
        hctx.fillStyle = '#0a0c10';
        hctx.fillRect(0, 0, plotW, plotH);
      }
      history.current = c;
      writeCol.current = 0;
    }

    const hist = history.current;
    const hctx = hist.getContext('2d');
    if (!hctx) return;

    // One column per (span / width) seconds, so the time axis means what the
    // label says regardless of how fast frames happen to arrive.
    const columnMs = (span * 1000) / plotW;
    const now = performance.now();
    const frame = frameRef.current;

    if (frame && frame.bandsDb.length === plan.bands.length && now - lastPaint.current >= columnMs) {
      lastPaint.current = now;
      const col = writeCol.current;

      const fMin = plan.bands[0]?.flo ?? 20;
      const fMax = plan.bands[plan.bands.length - 1]?.fhi ?? 20000;
      const image = hctx.createImageData(1, plotH);
      const data = image.data;

      // Fill the column band by band rather than pixel by pixel: at 1/48 octave
      // there are more bands than pixels, and at 1/1 there are far fewer, so
      // painting each band's pixel range covers both without a resample.
      for (let i = 0; i < plan.bands.length; i++) {
        const b = plan.bands[i];
        const yTop = Math.floor((1 - freqToUnit(b.fhi, fMin, fMax)) * plotH);
        const yBot = Math.ceil((1 - freqToUnit(b.flo, fMin, fMax)) * plotH);
        const t = (frame.bandsDb[i] - bottom) / (top - bottom);
        const [r, g, bl] = heatRgba(t);
        for (let y = Math.max(0, yTop); y < Math.min(plotH, Math.max(yBot, yTop + 1)); y++) {
          const o = y * 4;
          data[o] = r;
          data[o + 1] = g;
          data[o + 2] = bl;
          data[o + 3] = 255;
        }
      }
      hctx.putImageData(image, col, 0);
      writeCol.current = (col + 1) % plotW;
    }

    // Present: the ring is unwrapped in two blits, oldest part first.
    ctx.clearRect(0, 0, width, height);
    const split = writeCol.current;
    ctx.drawImage(hist, split, 0, plotW - split, plotH, PAD_LEFT, 0, plotW - split, plotH);
    if (split > 0) {
      ctx.drawImage(hist, 0, 0, split, plotH, PAD_LEFT + (plotW - split), 0, split, plotH);
    }

    // Frequency labels down the left.
    ctx.font = '10px ui-monospace, SFMono-Regular, Menlo, monospace';
    ctx.textAlign = 'right';
    ctx.textBaseline = 'middle';
    ctx.fillStyle = 'rgba(255,255,255,0.45)';
    const fMin = plan.bands[0]?.flo ?? 20;
    const fMax = plan.bands[plan.bands.length - 1]?.fhi ?? 20000;
    for (const hz of [100, 1000, 10000]) {
      if (hz < fMin || hz > fMax) continue;
      const y = (1 - freqToUnit(hz, fMin, fMax)) * plotH;
      ctx.fillText(hz >= 1000 ? `${hz / 1000}k` : String(hz), PAD_LEFT - 6, y);
      ctx.strokeStyle = 'rgba(255,255,255,0.14)';
      ctx.beginPath();
      ctx.moveTo(PAD_LEFT, Math.round(y) + 0.5);
      ctx.lineTo(width, Math.round(y) + 0.5);
      ctx.stroke();
    }
  });

  return (
    <div className="tile-body">
      <canvas ref={canvas} className="fill" />
      <span className="tile-corner">{span}s</span>
    </div>
  );
}

export function SpectrographSettings({ tile }: { tile: Tile }) {
  const setTileOptions = useStore((s) => s.setTileOptions);
  const calibrated = useStore((s) => s.calibration != null);
  const opts = tile.options as Options;

  return (
    <>
      <label>
        Span (s)
        <input
          type="number"
          min={5}
          max={600}
          value={opts.span ?? 30}
          onChange={(e) => setTileOptions(tile.id, { span: Math.max(5, Number(e.target.value)) })}
        />
      </label>
      <label>
        Top
        <input
          type="number"
          step={5}
          value={opts.top ?? (calibrated ? 110 : -10)}
          onChange={(e) => setTileOptions(tile.id, { top: Number(e.target.value) })}
        />
      </label>
      <label>
        Bottom
        <input
          type="number"
          step={5}
          value={opts.bottom ?? (calibrated ? 30 : -90)}
          onChange={(e) => setTileOptions(tile.id, { bottom: Number(e.target.value) })}
        />
      </label>
    </>
  );
}
