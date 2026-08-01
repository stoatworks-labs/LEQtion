import { describe, expect, it } from 'vitest';

import {
  dbTicks,
  dbToUnit,
  formatHz,
  freqTicks,
  freqToUnit,
  heatRgba,
  unitToFreq,
  F_MAX,
  F_MIN,
} from './plot';

describe('frequency axis', () => {
  it('puts the ends of the range at the ends of the axis', () => {
    expect(freqToUnit(F_MIN)).toBeCloseTo(0, 10);
    expect(freqToUnit(F_MAX)).toBeCloseTo(1, 10);
  });

  it('is logarithmic, so every octave takes the same width', () => {
    // 100→200 and 1000→2000 are both one octave and must measure the same.
    const low = freqToUnit(200) - freqToUnit(100);
    const high = freqToUnit(2000) - freqToUnit(1000);
    expect(high).toBeCloseTo(low, 10);
  });

  it('round-trips', () => {
    for (const hz of [20, 63, 440, 1000, 6300, 20000]) {
      expect(unitToFreq(freqToUnit(hz))).toBeCloseTo(hz, 6);
    }
  });

  it('does not produce infinities at zero', () => {
    expect(freqToUnit(0)).toBe(0);
    expect(Number.isFinite(freqToUnit(0))).toBe(true);
  });
});

describe('level axis', () => {
  it('puts the top of the range at the top of the plot', () => {
    expect(dbToUnit(0, 0, -90)).toBeCloseTo(0, 10);
    expect(dbToUnit(-90, 0, -90)).toBeCloseTo(1, 10);
    expect(dbToUnit(-45, 0, -90)).toBeCloseTo(0.5, 10);
  });

  it('clamps rather than running off the canvas', () => {
    expect(dbToUnit(40, 0, -90)).toBe(0);
    expect(dbToUnit(-200, 0, -90)).toBe(1);
  });

  it('treats a non-finite level as the floor', () => {
    expect(dbToUnit(Number.NEGATIVE_INFINITY, 0, -90)).toBe(1);
    expect(dbToUnit(Number.NaN, 0, -90)).toBe(1);
  });
});

describe('ticks', () => {
  it('always labels the decades', () => {
    const ticks = freqTicks(300);
    const labelled = ticks.filter((t) => t.major).map((t) => t.hz);
    for (const hz of [20, 100, 1000, 10000]) expect(labelled).toContain(hz);
  });

  it('drops the minor ticks when there is no room for them', () => {
    const narrow = freqTicks(200);
    const wide = freqTicks(1400);
    expect(narrow.every((t) => t.major)).toBe(true);
    expect(wide.some((t) => !t.major)).toBe(true);
  });

  it('keeps ticks in order and inside the range', () => {
    const ticks = freqTicks(1400);
    for (let i = 1; i < ticks.length; i++) expect(ticks[i].hz).toBeGreaterThan(ticks[i - 1].hz);
    expect(ticks.every((t) => t.hz >= F_MIN && t.hz <= F_MAX)).toBe(true);
  });

  it('chooses a level step that fits the height', () => {
    const tall = dbTicks(0, -90, 700);
    const short = dbTicks(0, -90, 90);
    expect(tall.length).toBeGreaterThan(short.length);
    expect(short.length).toBeGreaterThanOrEqual(2);
    // Every tick must be inside the range.
    for (const v of tall) {
      expect(v).toBeLessThanOrEqual(0);
      expect(v).toBeGreaterThanOrEqual(-90);
    }
  });

  it('handles an inverted or empty range without hanging', () => {
    expect(dbTicks(-90, 0, 400)).toEqual([]);
    expect(dbTicks(0, 0, 400)).toEqual([]);
  });
});

describe('frequency labels', () => {
  it('reads the way an engineer says it', () => {
    expect(formatHz(20)).toBe('20');
    expect(formatHz(630)).toBe('630');
    expect(formatHz(1000)).toBe('1k');
    expect(formatHz(1500)).toBe('1.5k');
    expect(formatHz(10000)).toBe('10k');
    expect(formatHz(20000)).toBe('20k');
  });
});

describe('spectrograph colours', () => {
  it('gets monotonically lighter, so brighter always means louder', () => {
    let previous = -1;
    for (let t = 0; t <= 1.0001; t += 0.02) {
      const [r, g, b] = heatRgba(t);
      // Rec. 709 luma.
      const luma = 0.2126 * r + 0.7152 * g + 0.0722 * b;
      expect(luma).toBeGreaterThan(previous - 1e-6);
      previous = luma;
    }
  });

  it('clamps out-of-range values instead of wrapping', () => {
    expect(heatRgba(-5)).toEqual(heatRgba(0));
    expect(heatRgba(5)).toEqual(heatRgba(1));
  });

  it('stays inside the byte range', () => {
    for (let t = 0; t <= 1; t += 0.05) {
      for (const c of heatRgba(t)) {
        expect(c).toBeGreaterThanOrEqual(0);
        expect(c).toBeLessThanOrEqual(255);
      }
    }
  });
});
