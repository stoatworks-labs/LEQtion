import { useEffect, useState } from 'react';

import { isPrerelease } from '../lib/version';
import { useStore } from '../state/store';

import { CalibrationDialog } from './CalibrationDialog';
import { DeviceBar } from './DeviceBar';
import { ProjectBar } from './ProjectBar';
import { TileGrid } from './TileGrid';
import { Toolbar } from './Toolbar';

export function App() {
  const ready = useStore((s) => s.ready);
  const init = useStore((s) => s.init);
  const error = useStore((s) => s.error);
  const setError = useStore((s) => s.setError);
  const version = useStore((s) => s.version);
  const [calibrating, setCalibrating] = useState(false);

  useEffect(() => {
    void init();
  }, [init]);

  if (!ready) {
    return (
      <main className="app loading">
        <p>Starting LEQtion…</p>
      </main>
    );
  }

  return (
    <main className="app">
      {isPrerelease(version) && <PrereleaseBanner version={version} />}

      <ProjectBar />
      <DeviceBar onCalibrate={() => setCalibrating(true)} />
      <Toolbar />

      {error && (
        <div className="banner bad" role="alert">
          <span>{error}</span>
          <button type="button" className="icon" aria-label="Dismiss" onClick={() => setError(null)}>
            ✕
          </button>
        </div>
      )}

      <TileGrid />

      <footer className="app-foot">
        <span>
          LEQtion {version}
          {isPrerelease(version) && ' · pre-release'}
        </span>
        <span>
          Not a certified sound level meter. Levels are only sound pressure levels once calibrated
          against a hardware calibrator.
        </span>
      </footer>

      {calibrating && <CalibrationDialog onClose={() => setCalibrating(false)} />}
    </main>
  );
}

/**
 * Says, permanently, that this build is not the released one.
 *
 * Not dismissible, and that is the point. A pre-release of a *measurement* tool is
 * a different thing from a pre-release of most software: the risk is not that it
 * crashes, it is that someone reads a number off it, writes it down, and quotes it
 * later. That warning has to still be on screen at the moment the number is being
 * read, so it cannot be something clicked away an hour earlier.
 *
 * It appears purely because the version carries a semver pre-release identifier —
 * see `lib/version.ts`. There is no flag to remember to turn off.
 */
function PrereleaseBanner({ version }: { version: string }) {
  return (
    <div className="banner beta" role="status">
      <span>
        <strong>Pre-release · {version}</strong> — this is <strong>LEQtion NEXT</strong>,
        not the current release. It carries unfinished work from the system-tuning thread.
        Projects and shows saved here may not open in later builds, and anything you have to
        defend should be measured on the stable release.
      </span>
    </div>
  );
}
