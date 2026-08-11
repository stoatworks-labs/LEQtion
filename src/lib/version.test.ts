import { describe, expect, it } from 'vitest';

import { isPrerelease, prereleaseLabel } from './version';

describe('isPrerelease', () => {
  it('recognises a semver pre-release identifier', () => {
    expect(isPrerelease('0.2.0-beta.1')).toBe(true);
    expect(isPrerelease('1.0.0-rc.3')).toBe(true);
    expect(isPrerelease('0.2.0-alpha')).toBe(true);
  });

  /**
   * The one that matters. A stable build must never show the pre-release warning:
   * a banner that cries wolf on a release someone is quoting numbers from is worse
   * than no banner at all.
   */
  it('says no for a plain release', () => {
    expect(isPrerelease('0.1.1')).toBe(false);
    expect(isPrerelease('1.0.0')).toBe(false);
    expect(isPrerelease('')).toBe(false);
  });

  it('is not fooled by a stray dash', () => {
    expect(isPrerelease('0.2.0-')).toBe(false);
    expect(isPrerelease('-0.2.0')).toBe(false);
  });
});

describe('prereleaseLabel', () => {
  it('is the identifier without the version', () => {
    expect(prereleaseLabel('0.2.0-beta.1')).toBe('beta.1');
    expect(prereleaseLabel('0.1.1')).toBe('');
  });
});
