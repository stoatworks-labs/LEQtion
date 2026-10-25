import { describe, expect, it } from 'vitest';

import {
  describeWindow,
  formatBalance,
  formatCorrelation,
  formatElapsed,
  formatHz,
  formatLevel,
  inputName,
  inputsName,
  levelName,
  NO_READING,
} from './format';

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
    // `leq::tests::a_periodic_leq_is_named_so_it_cannot_pass_for_a_sliding_one`
    expect(levelName('a', { kind: 'periodic', seconds: 900 })).toBe('LAeq,15min periods');
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
    // The Rust `LeqWindow::label` for the same window.
    expect(describeWindow({ kind: 'periodic', seconds: 900 })).toBe('every 15 min');
  });
});

describe('inputs', () => {
  it('counts from one, as the device bar does', () => {
    expect(inputName({ kind: 'channel', index: 0 })).toBe('In 1');
    expect(inputName({ kind: 'mix' })).toBe('Mix');
    expect(inputsName([2, 3])).toBe('In 3+4');
    expect(inputsName([])).toBe('none');
  });
});

describe('formatCorrelation', () => {
  it('always shows the sign, so a negative reading cannot pass for a positive one', () => {
    expect(formatCorrelation(0.834)).toBe('+0.83');
    expect(formatCorrelation(-0.41)).toBe('\u22120.41');
    expect(formatCorrelation(0.001)).toBe('0.00');
    expect(formatCorrelation(-0.001)).toBe('0.00');
    expect(formatCorrelation(null)).toBe('-.--');
  });
});

describe('formatBalance', () => {
  it('names the louder side', () => {
    expect(formatBalance(2.14)).toBe('L 2.1 dB');
    expect(formatBalance(-0.4)).toBe('R 0.4 dB');
    expect(formatBalance(0.01)).toBe('centre');
    expect(formatBalance(null)).toBe('—');
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

describe('formatHz', () => {
  it('reads to three significant figures with the unit chosen after rounding', () => {
    expect(formatHz(63.24)).toBe('63.2 Hz');
    expect(formatHz(997.3)).toBe('997 Hz');
    expect(formatHz(999.6)).toBe('1.00 kHz');
    expect(formatHz(2468)).toBe('2.47 kHz');
    expect(formatHz(12480)).toBe('12.5 kHz');
    expect(formatHz(19.686)).toBe('19.7 Hz');
  });

  it('shows a placeholder when there is nothing to name', () => {
    expect(formatHz(null)).toBe('—');
    expect(formatHz(undefined)).toBe('—');
    expect(formatHz(Number.NaN)).toBe('—');
  });
});
