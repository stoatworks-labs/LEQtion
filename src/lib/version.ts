/**
 * Is this build a pre-release?
 *
 * Semver says anything after a `-` is a pre-release identifier, so `0.2.0-beta.1`
 * is one and `0.2.0` is not. The app reads its own version from the crate at
 * startup, which means the pre-release warning appears and disappears on its own
 * as the version changes — there is no flag to forget to turn off before a real
 * release, which is exactly how a beta warning ends up shipping on a stable build.
 */
export function isPrerelease(version: string): boolean {
  const dash = version.indexOf('-');
  // A `-` with nothing after it is not an identifier, and a leading one is not a
  // version at all.
  return dash > 0 && dash < version.length - 1;
}

/** The pre-release label alone: `beta.1` from `0.2.0-beta.1`. */
export function prereleaseLabel(version: string): string {
  return isPrerelease(version) ? version.slice(version.indexOf('-') + 1) : '';
}
