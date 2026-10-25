/**
 * Level and duration formatting shared by the readout tiles.
 *
 * `levelName` deliberately mirrors `LeqSpec::derived_label` in `leqtion-dsp`.
 * Both sides generate it because the backend needs it for a reading it sends
 * unprompted and the frontend needs it before any frame has arrived — but they
 * must agree exactly, or a tile's heading and its own settings row would call
 * the same LEQ two different things. `format.test.ts` pins the cases.
 */
import type { ChannelSelect, LeqWindow, Weighting } from '../types';

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

/**
 * A frequency readout to three significant figures, with its unit: "63.2 Hz",
 * "997 Hz", "2.47 kHz", "12.5 kHz". Rounded before the unit is chosen, so
 * 999.6 Hz reads "1.00 kHz" rather than "1000 Hz".
 */
export function formatHz(hz: number | undefined | null): string {
  if (hz == null || !Number.isFinite(hz) || hz <= 0) return '—';
  const v = Number(hz.toPrecision(3));
  if (v >= 1000) return `${(v / 1000).toPrecision(3)} kHz`;
  return `${v.toPrecision(3)} Hz`;
}

/** Drop a trailing `.0`, and never show more than two decimals. */
function trim(v: number): string {
  return Number.isInteger(v) ? String(v) : String(+v.toFixed(2));
}

/** How a window reads in prose: "5 min", "125 ms", "since reset", "every 15 min". */
export function describeWindow(w: LeqWindow): string {
  if (w.kind === 'elapsed') return 'since reset';
  const s = w.seconds;
  const length =
    s < 1
      ? `${Math.round(s * 1000)} ms`
      : s < 60
        ? `${trim(s)} s`
        : s < 3600
          ? `${trim(s / 60)} min`
          : `${trim(s / 3600)} h`;
  return w.kind === 'periodic' ? `every ${length}` : length;
}

/** The compact form used inside a level name: `5min`, `125ms`, `1h`. */
function compactWindow(seconds: number): string {
  if (seconds < 1) return `${Math.round(seconds * 1000)}ms`;
  if (seconds < 60) return `${trim(seconds)}s`;
  if (seconds < 3600) return `${trim(seconds / 60)}min`;
  return `${trim(seconds / 3600)}h`;
}

/**
 * `LAeq,5min`, `LCeq`, `LZeq,125ms`, and `LAeq,15min periods` for consecutive
 * fixed periods — named apart from the sliding window of the same length,
 * because the two give different numbers.
 */
export function levelName(weighting: Weighting, window: LeqWindow): string {
  const w = weighting.toUpperCase();
  if (window.kind === 'elapsed') return `L${w}eq`;
  const name = `L${w}eq,${compactWindow(window.seconds)}`;
  return window.kind === 'periodic' ? `${name} periods` : name;
}

/** An input as the device bar numbers them: "In 1", counted from one, or "Mix". */
export function inputName(select: ChannelSelect): string {
  return select.kind === 'mix' ? 'Mix' : `In ${select.index + 1}`;
}

/** A list of inputs: "In 3+4". */
export function inputsName(inputs: number[]): string {
  return inputs.length === 0 ? 'none' : `In ${inputs.map((i) => i + 1).join('+')}`;
}


/** A correlation with its sign always shown: "+0.83", "−0.41", "0.00". */
export function formatCorrelation(r: number | undefined | null): string {
  if (r == null || !Number.isFinite(r)) return '-.--';
  const v = r.toFixed(2);
  if (v === '0.00' || v === '-0.00') return '0.00';
  return r > 0 ? `+${v}` : `\u2212${v.slice(1)}`;
}

/**
 * Which side is louder and by how much: "L 2.1 dB", "R 0.4 dB", "centre". The
 * number comes from the engine; this only says which way round it is.
 */
export function formatBalance(db: number | undefined | null): string {
  if (db == null || !Number.isFinite(db)) return '—';
  if (Math.abs(db) < 0.05) return 'centre';
  return `${db > 0 ? 'L' : 'R'} ${Math.abs(db).toFixed(1)} dB`;
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
