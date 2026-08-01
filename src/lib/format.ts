/**
 * Level and duration formatting shared by the readout tiles.
 *
 * `levelName` deliberately mirrors `LeqSpec::derived_label` in `leqtion-dsp`.
 * Both sides generate it because the backend needs it for a reading it sends
 * unprompted and the frontend needs it before any frame has arrived — but they
 * must agree exactly, or a tile's heading and its own settings row would call
 * the same LEQ two different things. `format.test.ts` pins the cases.
 */
import type { LeqWindow, Weighting } from '../types';

/**
 * A level, or a placeholder when there is nothing meaningful to show.
 *
 * The placeholder is ASCII hyphens rather than en-dashes on purpose. At the
 * 56 px monospace size the SPL tile uses, an en-dash sits high and thin and
 * reads as three floating bars rather than as "no reading".
 */
export const NO_READING = '--.-';

export function formatLevel(v: number | undefined | null): string {
  if (v == null || !Number.isFinite(v) || v <= -199) return NO_READING;
  return v.toFixed(1);
}

/** Drop a trailing `.0`, and never show more than two decimals. */
function trim(v: number): string {
  return Number.isInteger(v) ? String(v) : String(+v.toFixed(2));
}

/** How a window reads in prose: "5 min", "125 ms", "since reset". */
export function describeWindow(w: LeqWindow): string {
  if (w.kind === 'elapsed') return 'since reset';
  const s = w.seconds;
  if (s < 1) return `${Math.round(s * 1000)} ms`;
  if (s < 60) return `${trim(s)} s`;
  if (s < 3600) return `${trim(s / 60)} min`;
  return `${trim(s / 3600)} h`;
}

/** The compact form used inside a level name: `5min`, `125ms`, `1h`. */
function compactWindow(seconds: number): string {
  if (seconds < 1) return `${Math.round(seconds * 1000)}ms`;
  if (seconds < 60) return `${trim(seconds)}s`;
  if (seconds < 3600) return `${trim(seconds / 60)}min`;
  return `${trim(seconds / 3600)}h`;
}

/** `LAeq,5min`, `LCeq`, `LZeq,125ms`. */
export function levelName(weighting: Weighting, window: LeqWindow): string {
  const w = weighting.toUpperCase();
  if (window.kind === 'elapsed') return `L${w}eq`;
  return `L${w}eq,${compactWindow(window.seconds)}`;
}

/** Elapsed time as `m:ss`, or `h:mm:ss` once it runs past an hour. */
export function formatElapsed(seconds: number): string {
  const s = Math.max(0, Math.floor(seconds));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = s % 60;
  const pad = (n: number) => String(n).padStart(2, '0');
  return h > 0 ? `${h}:${pad(m)}:${pad(sec)}` : `${m}:${pad(sec)}`;
}
