import { useRef } from 'react';

import { formatBalance, formatCorrelation, inputsName } from '../lib/format';
import { api } from '../lib/ipc';
import { autoZoom, fitCanvas, SCOPE_ZOOM_LABEL, SCOPE_ZOOMS } from '../lib/plot';
import { useAnimationFrame, useFrameRef, useFrameValue } from '../lib/useFrame';
import { useStore } from '../state/store';
import type { Tile } from '../state/store';
import type { StereoSignal } from '../types';

/**
 * The vectorscope (goniometer) and the correlation meter under it.
 *
 * Every point is a true L/R pair from the engine, already rotated so mono is
 * vertical and scaled so the diamond is full scale (`leqtion-dsp::stereo`).
 * The tile only maps them to pixels and lets them fade.
 *
 * ## Persistence is time, not frames
 *
 * The phosphor layer fades by `exp(−dt/τ)` per repaint, so the afterglow lasts
 * the same on a 60 Hz and a 120 Hz display. Points are pulled with a cursor, so
 * each one is drawn once: a cloud that is brighter where the signal spends more
 * time, which is what the eye reads a vectorscope for.
 *
 * ## The zoom is always on screen
 *
 * A desk feed at −18 dBFS is a small blob in the middle at full scale, so the
 * tile zooms — automatically by default, in 6 dB steps — and prints the step.
 * A zoomed trace that did not say so would read as a louder signal. The
 * diamond is the edge of the plot at that zoom; at ×1 it is full scale.
 */

interface Options {
  zoom?: number | 'auto';
  persistence?: 'off' | 'short' | 'long';
}

/** Afterglow time constants, seconds. */
const PERSISTENCE: Record<NonNullable<Options['persistence']>, number> = {
  off: 0,
  short: 0.15,
  long: 0.8,
};

/** Points asked for per pull. More than one repaint's worth at any rate. */
const MAX_POINTS = 4096;
const PAD = 16;
const DOT = 'rgba(120, 225, 170, 0.55)';

export function VectorscopeTile({ tile }: { tile: Tile }) {
  const canvas = useRef<HTMLCanvasElement>(null);
  const phosphor = useRef<HTMLCanvasElement | null>(null);
  const cursor = useRef(0);
  const pending = useRef<number[]>([]);
  const inFlight = useRef(false);
  const zoomRef = useRef(1);
  const lastPaint = useRef(0);
  const frameRef = useFrameRef();
  const running = useStore((s) => s.status.running);

  const opts = tile.options as Options;
  const zoomSetting = opts.zoom ?? 'auto';
  const tau = PERSISTENCE[opts.persistence ?? 'short'];

  useAnimationFrame(() => {
    // Pull whatever has arrived since last time. One request at a time; the
    // cursor means nothing is lost while one is in flight.
    if (running && !inFlight.current) {
      inFlight.current = true;
      api
        .vectorscopePoints(cursor.current, MAX_POINTS)
        .then((p) => {
          cursor.current = p.next;
          pending.current = pending.current.length ? pending.current.concat(p.points) : p.points;
        })
        .catch(() => {
          /* the session may be between devices; the next pull tries again */
        })
        .finally(() => {
          inFlight.current = false;
        });
    }

    const el = canvas.current;
    if (!el) return;
    const ctx = el.getContext('2d');
    if (!ctx) return;
    const { width, height, changed } = fitCanvas(el);
    const dpr = window.devicePixelRatio || 1;

    if (!phosphor.current || changed) {
      const c = document.createElement('canvas');
      c.width = el.width;
      c.height = el.height;
      phosphor.current = c;
    }
    const pctx = phosphor.current.getContext('2d');
    if (!pctx) return;
    pctx.setTransform(dpr, 0, 0, dpr, 0, 0);

    const now = performance.now();
    const dt = lastPaint.current ? (now - lastPaint.current) / 1000 : 0;
    lastPaint.current = now;

    const frame = frameRef.current;
    const zoom =
      zoomSetting === 'auto' ? autoZoom(frame?.stereo.peak ?? 0, zoomRef.current) : zoomSetting;
    if (zoom !== zoomRef.current) {
      // Points drawn at the old scale would sit at the wrong radius under the
      // new one, so the afterglow goes with the change.
      zoomRef.current = zoom;
      pctx.clearRect(0, 0, width, height);
    }

    if (tau <= 0) {
      pctx.clearRect(0, 0, width, height);
    } else if (dt > 0) {
      pctx.globalCompositeOperation = 'destination-out';
      pctx.fillStyle = `rgba(0,0,0,${1 - Math.exp(-dt / tau)})`;
      pctx.fillRect(0, 0, width, height);
      pctx.globalCompositeOperation = 'source-over';
    }

    const half = Math.max(1, (Math.min(width, height) - 2 * PAD) / 2);
    const cx = width / 2;
    const cy = height / 2;
    const scale = half * zoom;

    const pts = pending.current;
    pending.current = [];
    pctx.fillStyle = DOT;
    for (let i = 0; i + 1 < pts.length; i += 2) {
      pctx.fillRect(cx + pts[i] * scale - 0.75, cy - pts[i + 1] * scale - 0.75, 1.5, 1.5);
    }

    ctx.clearRect(0, 0, width, height);
    drawGrid(ctx, cx, cy, half);
    ctx.drawImage(phosphor.current, 0, 0, width, height);
    drawLabels(ctx, cx, cy, half, zoom);
  });

  return (
    <div className="tile-body scope">
      <canvas ref={canvas} className="scope-canvas" />
      <ScopeStatus />
      <CorrelationMeter />
    </div>
  );
}

function drawGrid(ctx: CanvasRenderingContext2D, cx: number, cy: number, half: number) {
  ctx.lineWidth = 1;
  // The diamond: the edge of the plot, and full scale at ×1.
  ctx.strokeStyle = 'rgba(255,255,255,0.18)';
  ctx.beginPath();
  ctx.moveTo(cx, cy - half);
  ctx.lineTo(cx + half, cy);
  ctx.lineTo(cx, cy + half);
  ctx.lineTo(cx - half, cy);
  ctx.closePath();
  ctx.stroke();

  // Mono (vertical), out of phase (horizontal), and each side on its own.
  ctx.strokeStyle = 'rgba(255,255,255,0.08)';
  ctx.beginPath();
  ctx.moveTo(cx, cy - half);
  ctx.lineTo(cx, cy + half);
  ctx.moveTo(cx - half, cy);
  ctx.lineTo(cx + half, cy);
  ctx.moveTo(cx - half / 2, cy - half / 2);
  ctx.lineTo(cx + half / 2, cy + half / 2);
  ctx.moveTo(cx + half / 2, cy - half / 2);
  ctx.lineTo(cx - half / 2, cy + half / 2);
  ctx.stroke();
}

function drawLabels(ctx: CanvasRenderingContext2D, cx: number, cy: number, half: number, zoom: number) {
  ctx.font = '10px ui-monospace, SFMono-Regular, Menlo, monospace';
  ctx.fillStyle = 'rgba(255,255,255,0.45)';
  ctx.textBaseline = 'middle';
  ctx.textAlign = 'center';
  ctx.fillText('M', cx, cy - half - 8);
  ctx.textAlign = 'right';
  ctx.fillText('L', cx - half / 2 - 4, cy - half / 2 - 4);
  ctx.textAlign = 'left';
  ctx.fillText('R', cx + half / 2 + 4, cy - half / 2 - 4);
  ctx.fillText('S', cx + half + 4, cy);

  ctx.textAlign = 'left';
  ctx.textBaseline = 'top';
  ctx.fillStyle = zoom > 1 ? '#f5a524' : 'rgba(255,255,255,0.45)';
  ctx.fillText(SCOPE_ZOOM_LABEL[zoom] ?? `×${zoom}`, 6, 4);
}

const SIGNAL_TEXT: Record<StereoSignal, string> = {
  none: 'No signal on either side',
  leftOnly: 'Right side silent',
  rightOnly: 'Left side silent',
  both: '',
};

/** Which inputs, and anything about them worth saying. */
function ScopeStatus() {
  const stereo = useFrameValue(250)?.stereo;
  if (!stereo) return null;
  const note =
    stereo.left === stereo.right
      ? `both sides are input ${stereo.left + 1}`
      : SIGNAL_TEXT[stereo.signal];
  return (
    <span className="tile-corner">
      {inputsName([stereo.left, stereo.right])}
      {note && <span className="scope-note"> · {note}</span>}
    </span>
  );
}

/**
 * −1 to +1, with the lowest since reset held as a tick: whether the mono
 * fold-down ever cancelled, which the needle cannot say after the fact.
 */
function CorrelationMeter() {
  const stereo = useFrameValue(100)?.stereo;
  const r = stereo?.correlation ?? null;
  const min = stereo?.correlationMin ?? null;
  const at = (v: number) => `${((v + 1) / 2) * 100}%`;

  return (
    <div className="scope-meter">
      <span className="scope-meter-end">−1</span>
      <div className="corr-track" title="Phase correlation: +1 mono, 0 unrelated, −1 cancels in mono">
        <span className="corr-zero" />
        {min != null && <span className="corr-min" style={{ left: at(min) }} title="Lowest since reset" />}
        {r != null && <span className={r < 0 ? 'corr-needle bad' : 'corr-needle'} style={{ left: at(r) }} />}
      </div>
      <span className="scope-meter-end">+1</span>
      <span className={r != null && r < 0 ? 'corr-value bad' : 'corr-value'}>{formatCorrelation(r)}</span>
      <span className="corr-min-value" title="Lowest since reset">
        min {formatCorrelation(min)}
      </span>
      <span className="scope-balance" title="Which side is louder">
        {formatBalance(stereo?.balanceDb)}
      </span>
    </div>
  );
}

export function VectorscopeSettings({ tile }: { tile: Tile }) {
  const setTileOptions = useStore((s) => s.setTileOptions);
  const opts = tile.options as Options;
  return (
    <>
      <label>
        Zoom
        <select
          value={String(opts.zoom ?? 'auto')}
          onChange={(e) =>
            setTileOptions(tile.id, {
              zoom: e.target.value === 'auto' ? 'auto' : Number(e.target.value),
            })
          }
        >
          <option value="auto">Automatic</option>
          {SCOPE_ZOOMS.map((z) => (
            <option key={z} value={z}>
              {SCOPE_ZOOM_LABEL[z]}
            </option>
          ))}
        </select>
      </label>
      <label>
        Afterglow
        <select
          value={opts.persistence ?? 'short'}
          onChange={(e) => setTileOptions(tile.id, { persistence: e.target.value })}
        >
          <option value="off">Off</option>
          <option value="short">Short</option>
          <option value="long">Long</option>
        </select>
      </label>
      <p className="hint">The inputs are chosen under Inputs in the toolbar.</p>
    </>
  );
}
