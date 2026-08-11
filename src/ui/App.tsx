import { useEffect, useState } from 'react';

import { useFrameValue } from '../lib/useFrame';
import { isPrerelease } from '../lib/version';
import { useStore } from '../state/store';

import { CalibrationDialog } from './CalibrationDialog';
import { DeviceBar } from './DeviceBar';
import { ErrorBoundary } from './ErrorBoundary';
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
      {/* The toolbar owns the expanding panels, which is where a render fault
          is most likely and where losing the tiles would hurt most. */}
      <ErrorBoundary label="The toolbar">
        <Toolbar />
      </ErrorBoundary>

      <SilentInputBanner />

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

      {calibrating && (
        <ErrorBoundary label="The calibration dialog">
          <CalibrationDialog onClose={() => setCalibrating(false)} />
        </ErrorBoundary>
      )}
    </main>
  );
}

/** Seconds of unbroken digital silence before the input is called broken. */
const SILENCE_WARN_SECONDS = 3;

/**
 * Says that the input is delivering digital silence rather than quiet audio.
 *
 * This is the failure that looks most like working software. A denied
 * microphone on macOS does not error and does not stop the stream: it opens,
 * the callback fires on schedule, no frames are dropped, and every sample is
 * zero. So the meter reads its floor, the RTA is flat and empty, the log fills
 * with rows, and nothing anywhere says why — the app looks like it is measuring
 * a silent room. AGENTS.md §5 records the symptom and `capture.rs` has always
 * detected it on the command line; this is the same check where someone using
 * the app will actually see it.
 *
 * Suppressed on the generator backend, where an unbroken run of zeros is just
 * `Signal::Off` doing exactly what it should.
 */
function SilentInputBanner() {
  const running = useStore((s) => s.status.running);
  const host = useStore((s) => s.status.stream?.host);
  const frame = useFrameValue(500);

  const synthetic = host?.toLowerCase() === 'generator';
  const silent = frame?.inputSilentSeconds ?? 0;
  if (!running || synthetic || silent < SILENCE_WARN_SECONDS) return null;

  return (
    <div className="banner bad" role="alert">
      <span>
        <strong>The input is digital silence</strong> — every sample has been exactly zero for{' '}
        {Math.floor(silent)} s. This is not a quiet room; nothing is arriving. On macOS it usually
        means microphone access was denied, in System Settings → Privacy &amp; Security →
        Microphone. It can also mean nothing is connected to the selected channel.
      </span>
    </div>
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
