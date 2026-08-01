import { useEffect, useState } from 'react';

import { useStore } from '../state/store';

import { CalibrationDialog } from './CalibrationDialog';
import { DeviceBar } from './DeviceBar';
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
        <span>LEQtion {version}</span>
        <span>
          Not a certified sound level meter. Levels are only sound pressure levels once calibrated
          against a hardware calibrator.
        </span>
      </footer>

      {calibrating && <CalibrationDialog onClose={() => setCalibrating(false)} />}
    </main>
  );
}
