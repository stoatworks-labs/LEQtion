import { useRef, useState } from 'react';

import { api, errorText } from '../lib/ipc';
import { dbTicks, dbToUnit, fitCanvas, freqTicks, freqToUnit } from '../lib/plot';
import { useAnimationFrame, useFrameRef } from '../lib/useFrame';
import { useStore } from '../state/store';
import type { Tile } from '../state/store';
import type { DelayEstimate } from '../types';

/**
 * Magnitude, phase and coherence on one set of axes.
 *
 * ## Coherence is drawn, not hidden
 *
 * Every point is faded in proportion to its coherence, and points below the
 * floor are dropped entirely. This is the single most important thing on the
 * tile. A transfer function without coherence is a curve that looks equally
 * confident where the measurement is solid and where it is picking up the air
 * conditioning, and people tune systems off the second kind. Fading means the
 * trace visibly falls apart exactly where it should not be believed.
 *
 * ## Phase is plotted against its own axis
 *
 * Magnitude in dB down the left, phase in degrees down the right, over ±180°.
 * Phase is drawn as dots rather than a line: it wraps, and joining a point at
 * +179° to the next at −179° draws a vertical stripe across the whole plot that
 * reads as a feature and is an artefact of the wrap.
 */

interface Options {
  top?: number;
  bottom?: number;
  showPhase?: boolean;
  showCoherence?: boolean;
}

const PAD_LEFT = 42;
const PAD_RIGHT = 40;
const PAD_BOTTOM = 20;
const PAD_TOP = 8;

export function TransferTile({ tile }: { tile: Tile }) {
  const canvas = useRef<HTMLCanvasElement>(null);
  const frameRef = useFrameRef();
  const plan = useStore((s) => s.transferPlan);
  const config = useStore((s) => s.transfer);
  const reference = useStore((s) => s.status.reference);

  const opts = tile.options as Options;
  const top = opts.top ?? 12;
  const bottom = opts.bottom ?? -36;
  const showPhase = opts.showPhase ?? true;
  const showCoherence = opts.showCoherence ?? true;

  useAnimationFrame(() => {
    const el = canvas.current;
    if (!el || !plan) return;
    const ctx = el.getContext('2d');
    if (!ctx) return;

    const { width, height } = fitCanvas(el);
    const plotW = Math.max(1, width - PAD_LEFT - PAD_RIGHT);
    const plotH = Math.max(1, height - PAD_TOP - PAD_BOTTOM);
    ctx.clearRect(0, 0, width, height);

    const fMin = config.fMin;
    const fMax = config.fMax;
    const x = (hz: number) => PAD_LEFT + freqToUnit(hz, fMin, fMax) * plotW;
    const y = (db: number) => PAD_TOP + dbToUnit(db, top, bottom) * plotH;
    const yPhase = (deg: number) => PAD_TOP + ((180 - deg) / 360) * plotH;

    ctx.font = '10px ui-monospace, SFMono-Regular, Menlo, monospace';
    ctx.textBaseline = 'middle';

    for (const v of dbTicks(top, bottom, plotH)) {
      const py = Math.round(y(v)) + 0.5;
      ctx.strokeStyle = v === 0 ? 'rgba(255,255,255,0.22)' : 'rgba(255,255,255,0.08)';
      ctx.beginPath();
      ctx.moveTo(PAD_LEFT, py);
      ctx.lineTo(width - PAD_RIGHT, py);
      ctx.stroke();
      ctx.fillStyle = 'rgba(255,255,255,0.42)';
      ctx.textAlign = 'right';
      ctx.fillText(String(v), PAD_LEFT - 6, py);
    }

    if (showPhase) {
      ctx.fillStyle = 'rgba(240,180,90,0.55)';
      ctx.textAlign = 'left';
      for (const deg of [-180, -90, 0, 90, 180]) {
        ctx.fillText(`${deg}`, width - PAD_RIGHT + 5, yPhase(deg));
      }
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

    const tf = frameRef.current?.transfer;
    if (!tf || tf.magnitudeDb.length !== plan.frequencies.length) return;

    // Coherence, as a filled area along the bottom, before anything else.
    if (showCoherence) {
      ctx.fillStyle = 'rgba(90,200,150,0.16)';
      ctx.beginPath();
      ctx.moveTo(x(plan.frequencies[0]), PAD_TOP + plotH);
      for (let i = 0; i < plan.frequencies.length; i++) {
        ctx.lineTo(
          x(plan.frequencies[i]),
          PAD_TOP + plotH - tf.coherence[i] * plotH * 0.22,
        );
      }
      ctx.lineTo(x(plan.frequencies[plan.frequencies.length - 1]), PAD_TOP + plotH);
      ctx.closePath();
      ctx.fill();
    }

    // Phase first, so magnitude draws over it.
    if (showPhase) {
      for (let i = 0; i < plan.frequencies.length; i++) {
        const c = tf.coherence[i];
        if (c < config.coherenceFloor) continue;
        const p = tf.phaseDeg[i];
        if (!Number.isFinite(p)) continue;
        ctx.fillStyle = `rgba(240,180,90,${(0.25 + 0.65 * c).toFixed(3)})`;
        ctx.fillRect(x(plan.frequencies[i]) - 1, yPhase(p) - 1, 2, 2);
      }
    }

    // Magnitude, broken wherever coherence drops below the floor so the trace
    // does not stride confidently across a region it knows nothing about.
    ctx.lineWidth = 1.6;
    ctx.lineJoin = 'round';
    let drawing = false;
    for (let i = 0; i < plan.frequencies.length; i++) {
      const c = tf.coherence[i];
      const m = tf.magnitudeDb[i];
      const usable = c >= config.coherenceFloor && Number.isFinite(m);
      if (!usable) {
        if (drawing) {
          ctx.stroke();
          drawing = false;
        }
        continue;
      }
      const px = x(plan.frequencies[i]);
      const py = y(m);
      if (!drawing) {
        ctx.strokeStyle = '#2f9ee0';
        ctx.beginPath();
        ctx.moveTo(px, py);
        drawing = true;
      } else {
        ctx.lineTo(px, py);
      }
    }
    if (drawing) ctx.stroke();

    ctx.strokeStyle = 'rgba(255,255,255,0.18)';
    ctx.beginPath();
    ctx.moveTo(PAD_LEFT + 0.5, PAD_TOP);
    ctx.lineTo(PAD_LEFT + 0.5, PAD_TOP + plotH + 0.5);
    ctx.lineTo(width - PAD_RIGHT, PAD_TOP + plotH + 0.5);
    ctx.stroke();
  });

  return (
    <div className="tile-body">
      <canvas ref={canvas} className="fill" />
      {reference.kind === 'off' && (
        <p className="tile-empty overlay">
          No reference selected. Choose one in this tile&rsquo;s settings — the generator&rsquo;s
          internal tap, or an input carrying a loopback.
        </p>
      )}
      {!plan && <p className="tile-empty overlay">Waiting for a transfer plan…</p>}
    </div>
  );
}

export function TransferSettings({ tile }: { tile: Tile }) {
  const setTileOptions = useStore((s) => s.setTileOptions);
  const transfer = useStore((s) => s.transfer);
  const setTransfer = useStore((s) => s.setTransfer);
  const resetTransfer = useStore((s) => s.resetTransfer);
  const reference = useStore((s) => s.status.reference);
  const setReference = useStore((s) => s.setReference);
  const status = useStore((s) => s.status);
  const plan = useStore((s) => s.transferPlan);
  const opts = tile.options as Options;

  const [delay, setDelay] = useState<DelayEstimate | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const inputChannels = status.stream?.channels ?? 2;

  async function find() {
    setBusy(true);
    setError(null);
    try {
      const est = await api.findDelay();
      setDelay(est);
      if (!est) setError('Not enough signal yet — let the measurement run for a second.');
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <>
      <label>
        Reference
        <select
          value={reference.kind === 'loopback' ? `loopback:${reference.channel}` : reference.kind}
          onChange={(e) => {
            const v = e.target.value;
            void setReference(
              v.startsWith('loopback:')
                ? { kind: 'loopback', channel: Number(v.split(':')[1]) }
                : v === 'internal'
                  ? { kind: 'internal' }
                  : { kind: 'off' },
            );
          }}
        >
          <option value="off">Off</option>
          <option value="internal">Generator (internal)</option>
          {Array.from({ length: Math.max(1, inputChannels) }, (_, i) => (
            <option key={i} value={`loopback:${i}`}>
              Loopback on input {i + 1}
            </option>
          ))}
        </select>
      </label>

      <label>
        Points/octave
        <select
          value={transfer.pointsPerOctave}
          onChange={(e) =>
            void setTransfer((t) => ({ ...t, pointsPerOctave: Number(e.target.value) }))
          }
        >
          {[6, 12, 24, 48].map((n) => (
            <option key={n} value={n}>
              {n}
            </option>
          ))}
        </select>
      </label>

      <label>
        Averaging
        <select
          value={transfer.averaging}
          onChange={(e) =>
            void setTransfer((t) => ({ ...t, averaging: e.target.value as typeof t.averaging }))
          }
        >
          {(['fast', 'slow', 'long', 'infinite'] as const).map((a) => (
            <option key={a} value={a}>
              {a[0].toUpperCase() + a.slice(1)}
            </option>
          ))}
        </select>
      </label>

      <label>
        Coherence floor
        <input
          type="number"
          min={0}
          max={1}
          step={0.05}
          value={transfer.coherenceFloor}
          onChange={(e) =>
            void setTransfer((t) => ({ ...t, coherenceFloor: Number(e.target.value) }))
          }
        />
      </label>

      <label>
        Top
        <input
          type="number"
          step={3}
          value={opts.top ?? 12}
          onChange={(e) => setTileOptions(tile.id, { top: Number(e.target.value) })}
        />
      </label>
      <label>
        Bottom
        <input
          type="number"
          step={3}
          value={opts.bottom ?? -36}
          onChange={(e) => setTileOptions(tile.id, { bottom: Number(e.target.value) })}
        />
      </label>

      <label className="check">
        <input
          type="checkbox"
          checked={opts.showPhase ?? true}
          onChange={(e) => setTileOptions(tile.id, { showPhase: e.target.checked })}
        />
        Phase
      </label>
      <label className="check">
        <input
          type="checkbox"
          checked={opts.showCoherence ?? true}
          onChange={(e) => setTileOptions(tile.id, { showCoherence: e.target.checked })}
        />
        Coherence
      </label>

      <div className="delay-tools">
        <button type="button" disabled={busy || reference.kind === 'off'} onClick={() => void find()}>
          Find delay
        </button>
        {delay && (
          <>
            <span className="chip">
              {delay.milliseconds.toFixed(2)} ms · {delay.metres.toFixed(2)} m ·{' '}
              {Math.round(delay.samples)} samples
            </span>
            <button
              type="button"
              className="primary"
              onClick={() => {
                void api.setDelaySamples(Math.max(0, Math.round(delay.samples)));
                setDelay(null);
              }}
            >
              Apply
            </button>
            {delay.confidence < 0.2 && (
              <span className="chip warn">
                weak arrival — this may be a reflection rather than the direct sound
              </span>
            )}
          </>
        )}
        <button type="button" onClick={() => void resetTransfer()}>
          Restart averaging
        </button>
      </div>

      {error && <p className="tile-warn">{error}</p>}

      {status.referenceUnderruns > 0 && (
        <p className="tile-warn">
          The internal reference ran dry {status.referenceUnderruns} times, so its alignment
          is no longer trustworthy. Find the delay again.
        </p>
      )}

      {plan && (
        <p className="hint">
          {plan.frequencies.length} points · longest window{' '}
          {plan.longestWindowSeconds.toFixed(2)} s, which is how long the bottom of the range
          takes to settle.
        </p>
      )}
    </>
  );
}
