import { useRef } from 'react';

import { formatHz, formatLevel, inputName } from '../lib/format';
import { dbToUnit, dbTicks, fitCanvas, freqTicks, freqToUnit } from '../lib/plot';
import { useAnimationFrame, useFrameRef, useFrameValue, useSpectrumCalibrated } from '../lib/useFrame';
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
 *
 * The strip across the top names the tallest band and its level. The engine
 * picks the band and refines its frequency (`Frame.peakBand`); nothing is
 * derived here, so the readout can never disagree with the bars under it.
 *
 * ## The comparison trace
 *
 * When the routing names a comparison input, its spectrum is drawn as a line
 * over the bars, from the same band plan. The two are usually in different
 * units — the microphone calibrated, the desk feed not — and one axis cannot
 * carry both without presenting one of them as the other. So when the units
 * differ the line gets its own axis on the right, labelled in its own unit,
 * with the same number of decibels per pixel so shapes still compare.
 */

interface Options {
  top?: number;
  bottom?: number;
  showPeaks?: boolean;
  showCompare?: boolean;
  /** Top of the comparison trace's own axis, when its unit differs. */
  compareTop?: number;
}

const PAD_LEFT = 42;
const PAD_BOTTOM = 20;
const PAD_TOP = 8;
const PAD_RIGHT = 8;
/** Room for the comparison axis's labels, when it has its own. */
const PAD_RIGHT_AXIS = 40;
const COMPARE_COLOUR = '#f0a33a';

export function RtaTile({ tile }: { tile: Tile }) {
  const canvas = useRef<HTMLCanvasElement>(null);
  const frameRef = useFrameRef();
  const plan = useStore((s) => s.plan);
  const calibrated = useSpectrumCalibrated();

  const opts = tile.options as Options;
  // Uncalibrated levels are dBFS and sit below zero; calibrated ones are dB SPL
  // and sit well above it. Defaulting the axis per case saves everyone the first
  // action after every calibration.
  const top = opts.top ?? (calibrated ? 120 : 0);
  const bottom = opts.bottom ?? (calibrated ? 20 : -100);
  const showPeaks = opts.showPeaks ?? true;
  const showCompare = opts.showCompare ?? true;

  useAnimationFrame(() => {
    const el = canvas.current;
    if (!el || !plan) return;
    const ctx = el.getContext('2d');
    if (!ctx) return;

    const { width, height } = fitCanvas(el);
    const frame = frameRef.current;
    const compare = showCompare ? frame?.compare : undefined;
    // Its own axis only when its unit is not the bars' unit.
    const ownAxis = compare != null && compare.calibrated !== frame?.spectrumCalibrated;
    const padRight = ownAxis ? PAD_RIGHT_AXIS : PAD_RIGHT;
    const plotW = Math.max(1, width - PAD_LEFT - padRight);
    const plotH = Math.max(1, height - PAD_TOP - PAD_BOTTOM);
    const cTop = ownAxis ? (opts.compareTop ?? (compare?.calibrated ? 120 : 0)) : top;
    const cBottom = cTop - (top - bottom);

    ctx.clearRect(0, 0, width, height);

    const fMin = plan.bands[0]?.flo ?? 20;
    const fMax = plan.bands[plan.bands.length - 1]?.fhi ?? 20000;
    const x = (hz: number) => PAD_LEFT + freqToUnit(hz, fMin, fMax) * plotW;
    const y = (db: number) => PAD_TOP + dbToUnit(db, top, bottom) * plotH;
    const yc = (db: number) => PAD_TOP + dbToUnit(db, cTop, cBottom) * plotH;

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
      ctx.lineTo(width - padRight, py);
      ctx.stroke();
      ctx.fillStyle = 'rgba(255,255,255,0.42)';
      ctx.textAlign = 'right';
      ctx.fillText(String(v), PAD_LEFT - 6, py);
    }
    if (ownAxis) {
      ctx.fillStyle = COMPARE_COLOUR;
      ctx.textAlign = 'left';
      for (const v of dbTicks(cTop, cBottom, plotH)) {
        ctx.fillText(String(v), width - padRight + 6, Math.round(yc(v)) + 0.5);
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

    // The comparison trace: a step line along the band edges, so each band's
    // level is drawn across exactly the width its bar would have.
    if (compare && compare.bandsDb.length === plan.bands.length) {
      ctx.strokeStyle = COMPARE_COLOUR;
      ctx.lineWidth = 1.5;
      ctx.beginPath();
      let drawing = false;
      for (let i = 0; i < plan.bands.length; i++) {
        const b = plan.bands[i];
        const level = compare.bandsDb[i];
        if (!Number.isFinite(level) || level <= -199) {
          drawing = false;
          continue;
        }
        const py = yc(level);
        if (drawing) ctx.lineTo(x(b.flo), py);
        else ctx.moveTo(x(b.flo), py);
        ctx.lineTo(x(b.fhi), py);
        drawing = true;
      }
      ctx.stroke();
      ctx.lineWidth = 1;
    }

    // Axis frame last, so bars never overdraw it.
    ctx.strokeStyle = 'rgba(255,255,255,0.18)';
    ctx.beginPath();
    ctx.moveTo(PAD_LEFT + 0.5, PAD_TOP);
    ctx.lineTo(PAD_LEFT + 0.5, baseline + 0.5);
    ctx.lineTo(width - padRight, baseline + 0.5);
    ctx.stroke();
  });

  return (
    <div className="tile-body rta">
      <RtaPeakBanner showCompare={showCompare} />
      <canvas ref={canvas} className="rta-canvas" />
      {!plan && <p className="tile-empty">Waiting for a band plan…</p>}
    </div>
  );
}

/**
 * The peak readout, in its own component so that its ten-a-second re-render
 * does not take the canvas with it. The level is the band's own bar height,
 * read straight out of the frame, and the unit follows the calibration
 * exactly as it does on the SPL tile.
 */
function RtaPeakBanner({ showCompare }: { showCompare: boolean }) {
  const frame = useFrameValue(100);
  const peak = frame?.peakBand ?? null;
  const level = frame && peak ? frame.bandsDb[peak.index] : undefined;
  const calibrated = frame?.spectrumCalibrated ?? false;
  const compare = showCompare ? frame?.compare : undefined;
  const compareInput = frame?.inputs.compare;

  return (
    <div className="rta-head" title="The tallest band of the RTA, and its level">
      <span className="rta-peak-name">Peak</span>
      <span className="rta-peak-hz">{formatHz(peak?.hz)}</span>
      <span className="rta-peak-level">{formatLevel(level)}</span>
      <span className={calibrated ? 'rta-peak-unit' : 'rta-peak-unit uncal'}>
        {calibrated ? 'dB SPL' : 'dBFS'}
      </span>
      {frame && (
        <span className="rta-inputs">
          <span className="rta-swatch bars" />
          {inputName(frame.inputs.spectrum)}
          {compare && compareInput && (
            <>
              <span className="rta-swatch line" />
              {inputName(compareInput)}
              <span className={compare.calibrated ? 'rta-peak-unit' : 'rta-peak-unit uncal'}>
                {compare.calibrated ? 'dB SPL' : 'dBFS'}
              </span>
            </>
          )}
        </span>
      )}
    </div>
  );
}

export function RtaSettings({ tile }: { tile: Tile }) {
  const setTileOptions = useStore((s) => s.setTileOptions);
  const calibrated = useSpectrumCalibrated();
  const routedCompare = useStore((s) => s.config.routing.compare != null);
  const frame = useFrameValue(500);
  const opts = tile.options as Options;
  const top = opts.top ?? (calibrated ? 120 : 0);
  const bottom = opts.bottom ?? (calibrated ? 20 : -100);
  const compareOwnAxis =
    frame?.compare != null && frame.compare.calibrated !== frame.spectrumCalibrated;

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
      {routedCompare ? (
        <>
          <label className="check">
            <input
              type="checkbox"
              checked={opts.showCompare ?? true}
              onChange={(e) => setTileOptions(tile.id, { showCompare: e.target.checked })}
            />
            Show the comparison trace
          </label>
          {compareOwnAxis && (
            <label>
              Comparison top
              <input
                type="number"
                step={5}
                value={opts.compareTop ?? (frame?.compare?.calibrated ? 120 : 0)}
                onChange={(e) => setTileOptions(tile.id, { compareTop: Number(e.target.value) })}
              />
            </label>
          )}
        </>
      ) : (
        <p className="hint">
          A second input can be drawn over the bars as a line — pick one under Inputs in the
          toolbar.
        </p>
      )}
    </>
  );
}
