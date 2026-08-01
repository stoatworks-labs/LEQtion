/**
 * Axis maths and the spectrograph colour map.
 *
 * Pure functions, no canvas and no React, so the awkward parts — tick
 * selection, the log axis, the colour ramp — are testable without a browser.
 * Every tile that draws a frequency axis uses these, which is what stops the
 * RTA and the spectrograph disagreeing about where 1 kHz is.
 */

/** Bottom and top of the frequency axis, in Hz. */
export const F_MIN = 20;
export const F_MAX = 20000;

/** Map a frequency to a horizontal position, 0..1, on a log axis. */
export function freqToUnit(hz: number, fMin = F_MIN, fMax = F_MAX): number {
  if (hz <= 0) return 0;
  return Math.log2(hz / fMin) / Math.log2(fMax / fMin);
}

export function unitToFreq(u: number, fMin = F_MIN, fMax = F_MAX): number {
  return fMin * Math.pow(2, u * Math.log2(fMax / fMin));
}

/**
 * Map a level to a vertical position, 0..1, where 0 is the top of the plot.
 *
 * Clamped rather than allowed to run off: a band at −200 dB would otherwise
 * draw thousands of pixels below the canvas, and some browsers get slow rather
 * than clipping.
 */
export function dbToUnit(db: number, top: number, bottom: number): number {
  if (!Number.isFinite(db)) return 1;
  const u = (top - db) / (top - bottom);
  return Math.min(1, Math.max(0, u));
}

export interface Tick {
  hz: number;
  label: string;
  /** Major ticks get a label and a brighter line. */
  major: boolean;
}

const MAJOR_TICKS = [20, 50, 100, 200, 500, 1000, 2000, 5000, 10000, 20000];
const MINOR_TICKS = [
  30, 40, 60, 70, 80, 90, 150, 300, 400, 600, 700, 800, 900, 1500, 3000, 4000, 6000, 7000, 8000,
  9000, 15000,
];

export function formatHz(hz: number): string {
  if (hz >= 10000) return `${Math.round(hz / 1000)}k`;
  if (hz >= 1000) {
    const k = hz / 1000;
    return Number.isInteger(k) ? `${k}k` : `${k.toFixed(1)}k`;
  }
  return `${Math.round(hz)}`;
}

/**
 * Ticks for the frequency axis.
 *
 * Minor ticks are dropped when the plot is too narrow for them to be anything
 * but noise. The threshold is in pixels-per-decade rather than total width, so
 * a wide plot showing a narrow range keeps its detail.
 */
export function freqTicks(widthPx: number, fMin = F_MIN, fMax = F_MAX): Tick[] {
  const decades = Math.log10(fMax / fMin);
  const pxPerDecade = widthPx / Math.max(decades, 0.001);
  const ticks: Tick[] = [];

  for (const hz of MAJOR_TICKS) {
    if (hz >= fMin && hz <= fMax) ticks.push({ hz, label: formatHz(hz), major: true });
  }
  if (pxPerDecade > 220) {
    for (const hz of MINOR_TICKS) {
      if (hz >= fMin && hz <= fMax) ticks.push({ hz, label: '', major: false });
    }
  }
  ticks.sort((a, b) => a.hz - b.hz);
  return ticks;
}

/**
 * Ticks for the level axis, chosen so there are enough to read and few enough
 * to see through.
 */
export function dbTicks(top: number, bottom: number, heightPx: number): number[] {
  const span = top - bottom;
  if (span <= 0) return [];
  const wanted = Math.max(2, Math.min(12, Math.floor(heightPx / 34)));
  const raw = span / wanted;
  const steps = [1, 2, 5, 10, 20, 25, 50, 100];
  const step = steps.find((s) => s >= raw) ?? 100;

  const out: number[] = [];
  const first = Math.ceil(bottom / step) * step;
  for (let v = first; v <= top + 1e-9; v += step) out.push(Math.round(v * 100) / 100);
  return out;
}

/**
 * Spectrograph colour ramp, dark to hot.
 *
 * A perceptual-ish ramp rather than a hue rotation: a rainbow map invents
 * banding where the data is smooth, and the eye reads the yellow-green edge as
 * a feature that is not there. This one rises monotonically in lightness, so
 * "brighter" always means "louder" and nothing else.
 */
const RAMP: [number, number, number][] = [
  [8, 10, 16],
  [18, 32, 68],
  [22, 74, 120],
  [30, 128, 150],
  [80, 176, 130],
  [180, 205, 90],
  [245, 200, 70],
  [252, 240, 190],
];

/** `t` is 0..1. Returns packed 0xRRGGBB. */
export function heatColour(t: number): number {
  const u = Math.min(1, Math.max(0, t)) * (RAMP.length - 1);
  const i = Math.min(RAMP.length - 2, Math.floor(u));
  const f = u - i;
  const a = RAMP[i];
  const b = RAMP[i + 1];
  const r = Math.round(a[0] + (b[0] - a[0]) * f);
  const g = Math.round(a[1] + (b[1] - a[1]) * f);
  const bl = Math.round(a[2] + (b[2] - a[2]) * f);
  return (r << 16) | (g << 8) | bl;
}

export function heatRgba(t: number): [number, number, number] {
  const c = heatColour(t);
  return [(c >> 16) & 255, (c >> 8) & 255, c & 255];
}

/**
 * Size a canvas to its CSS box at the device pixel ratio.
 *
 * Returns false when nothing changed, so a caller can skip the redraw. Getting
 * this wrong is why canvas text looks soft on a retina display: the backing
 * store has to be in device pixels while the drawing commands stay in CSS
 * pixels, which is what the transform below arranges.
 */
export function fitCanvas(canvas: HTMLCanvasElement): { width: number; height: number; changed: boolean } {
  const dpr = window.devicePixelRatio || 1;
  const rect = canvas.getBoundingClientRect();
  const width = Math.max(1, Math.round(rect.width));
  const height = Math.max(1, Math.round(rect.height));
  const bw = Math.round(width * dpr);
  const bh = Math.round(height * dpr);
  const changed = canvas.width !== bw || canvas.height !== bh;
  if (changed) {
    canvas.width = bw;
    canvas.height = bh;
  }
  const ctx = canvas.getContext('2d');
  if (ctx) ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  return { width, height, changed };
}
