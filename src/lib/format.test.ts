import { describe, expect, it } from 'vitest';

import { describeWindow, formatElapsed, formatLevel, levelName, NO_READING } from './format';

describe('levelName', () => {
  /**
   * These are exactly the cases pinned on the Rust side in
   * `leq::tests::derived_labels_read_like_level_names`. If one list changes, the
   * other has to change with it.
   */
  it('matches the Rust derived_label for the same inputs', () => {
    expect(levelName('a', { kind: 'elapsed' })).toBe('LAeq');
    expect(levelName('a', { kind: 'sliding', seconds: 300 })).toBe('LAeq,5min');
    expect(levelName('z', { kind: 'sliding', seconds: 0.125 })).toBe('LZeq,125ms');
    expect(levelName('c', { kind: 'sliding', seconds: 3600 })).toBe('LCeq,1h');
    expect(levelName('c', { kind: 'sliding', seconds: 10 })).toBe('LCeq,10s');
  });

  it('does not leave a trailing zero on a whole number', () => {
    expect(levelName('a', { kind: 'sliding', seconds: 60 })).toBe('LAeq,1min');
    expect(levelName('a', { kind: 'sliding', seconds: 90 })).toBe('LAeq,1.5min');
  });
});

describe('formatLevel', () => {
  it('shows one decimal', () => {
    expect(formatLevel(94)).toBe('94.0');
    expect(formatLevel(103.26)).toBe('103.3');
    expect(formatLevel(-26.04)).toBe('-26.0');
  });

  it('shows a placeholder rather than a silence floor or a NaN', () => {
    // -200 dBFS is the engine's stand-in for digital silence; printing it as a
    // level would look like a measurement.
    expect(formatLevel(-200)).toBe(NO_READING);
    expect(formatLevel(Number.NEGATIVE_INFINITY)).toBe(NO_READING);
    expect(formatLevel(Number.NaN)).toBe(NO_READING);
    expect(formatLevel(undefined)).toBe(NO_READING);
    expect(formatLevel(null)).toBe(NO_READING);
  });
});

describe('describeWindow', () => {
  it('reads the way someone would say it', () => {
    expect(describeWindow({ kind: 'elapsed' })).toBe('since reset');
    expect(describeWindow({ kind: 'sliding', seconds: 0.125 })).toBe('125 ms');
    expect(describeWindow({ kind: 'sliding', seconds: 5 })).toBe('5 s');
    expect(describeWindow({ kind: 'sliding', seconds: 300 })).toBe('5 min');
    expect(describeWindow({ kind: 'sliding', seconds: 3600 })).toBe('1 h');
  });
});

describe('formatElapsed', () => {
  it('grows a field only when it needs one', () => {
    expect(formatElapsed(0)).toBe('0:00');
    expect(formatElapsed(9)).toBe('0:09');
    expect(formatElapsed(605)).toBe('10:05');
    expect(formatElapsed(3661)).toBe('1:01:01');
  });

  it('never shows a negative time', () => {
    expect(formatElapsed(-5)).toBe('0:00');
  });
});
