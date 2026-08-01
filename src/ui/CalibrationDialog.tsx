import { useEffect, useState } from 'react';

import { api, errorText } from '../lib/ipc';
import { useFrameValue } from '../lib/useFrame';
import { useStore } from '../state/store';
import type { CalibrationStatus, CalibrationTarget } from '../types';

/**
 * Calibrating against a hardware acoustic calibrator.
 *
 * The dialog exists to make a bad calibration hard to accept. The engine already
 * refuses one that is unsteady, at the wrong frequency, clipping or too quiet —
 * this is where the reason is explained in words a person can act on, because
 * "unstable" on its own does not tell anyone that the calibrator is not seated
 * on the capsule.
 *
 * Accept stays disabled until the run is genuinely ready. There is no override:
 * a calibration is trusted silently for the rest of the measurement, and every
 * number that follows inherits it.
 */
export function CalibrationDialog({ onClose }: { onClose: () => void }) {
  const targets = useStore((s) => s.calibrationTargets);
  const calibration = useStore((s) => s.calibration);
  const refreshCalibration = useStore((s) => s.refreshCalibration);
  const stream = useStore((s) => s.status.stream);

  const [target, setTarget] = useState<CalibrationTarget>(
    targets[0] ?? { levelDb: 94, frequencyHz: 1000 },
  );
  const [error, setError] = useState<string | null>(null);
  const frame = useFrameValue(150);
  const status = frame?.calibration ?? null;

  // Restarting the run on every target change is the point: the previous
  // seconds were measured against a different reference and cannot contribute.
  useEffect(() => {
    setError(null);
    void api.beginCalibration(target).catch((e) => setError(errorText(e)));
    return () => {
      void api.cancelCalibration().catch(() => {});
    };
  }, [target]);

  const ready = status?.state === 'ready';

  async function accept() {
    try {
      await api.acceptCalibration();
      await refreshCalibration();
      onClose();
    } catch (e) {
      setError(errorText(e));
    }
  }

  async function clear() {
    try {
      await api.clearCalibration();
      await refreshCalibration();
      onClose();
    } catch (e) {
      setError(errorText(e));
    }
  }

  return (
    <div className="modal-backdrop" role="dialog" aria-modal="true" aria-label="Calibrate">
      <div className="modal">
        <h2>Calibrate</h2>
        <p className="lede">
          Fit the calibrator over the capsule, switch it on, and wait for the level to settle.
          {stream ? ` Measuring ${stream.device}.` : ''}
        </p>

        <label>
          Calibrator output
          <select
            value={`${target.levelDb}/${target.frequencyHz}`}
            onChange={(e) => {
              const [lvl, hz] = e.target.value.split('/').map(Number);
              setTarget({ levelDb: lvl, frequencyHz: hz });
            }}
          >
            {targets.map((t) => (
              <option key={`${t.levelDb}/${t.frequencyHz}`} value={`${t.levelDb}/${t.frequencyHz}`}>
                {t.levelDb} dB at {t.frequencyHz} Hz
              </option>
            ))}
          </select>
        </label>

        <StatusPanel status={status} target={target} />

        {error && <p className="tile-warn">{error}</p>}

        <div className="modal-actions">
          {calibration && (
            <button type="button" onClick={() => void clear()}>
              Clear calibration
            </button>
          )}
          <span className="spacer" />
          <button type="button" onClick={onClose}>
            Cancel
          </button>
          <button type="button" className="primary" disabled={!ready} onClick={() => void accept()}>
            Accept
          </button>
        </div>
      </div>
    </div>
  );
}

function StatusPanel({
  status,
  target,
}: {
  status: CalibrationStatus | null;
  target: CalibrationTarget;
}) {
  if (!status) {
    return <p className="cal-status">Waiting for audio — is the input running?</p>;
  }

  switch (status.state) {
    case 'settling':
      return (
        <div className="cal-status">
          <div className="cal-progress">
            <span style={{ width: `${Math.round(status.progress * 100)}%` }} />
          </div>
          <p>Listening…</p>
        </div>
      );

    case 'unstable':
      return (
        <p className="cal-status warn">
          The level is still moving by {status.spreadDb.toFixed(2)} dB. Check the calibrator is
          seated squarely on the capsule and give it a few seconds to stabilise.
        </p>
      );

    case 'wrongFrequency':
      return (
        <p className="cal-status warn">
          Hearing {Math.round(status.measuredHz)} Hz, expecting {Math.round(status.expectedHz)} Hz.
          Either the calibrator is on its other setting, or the wrong output is selected above.
        </p>
      );

    case 'clipping':
      return (
        <p className="cal-status bad">
          The input is clipping, so the measured level is only a lower bound. Turn the preamp gain
          down and start again — and remember the calibration only holds for the gain it was taken
          at.
        </p>
      );

    case 'tooQuiet':
      return (
        <p className="cal-status warn">
          Only {status.levelDbfs.toFixed(1)} dBFS — far too quiet for a calibrator. Check it is
          switched on, that the right input channel is selected, and that phantom power is on if the
          microphone needs it.
        </p>
      );

    case 'ready':
      return (
        <div className="cal-status good">
          <p>
            Steady at <strong>{status.measuredDbfs.toFixed(2)} dBFS</strong>, moving by{' '}
            {status.spreadDb.toFixed(2)} dB.
          </p>
          <p>
            Accepting sets the offset to <strong>{status.offsetDb.toFixed(2)} dB</strong>, which
            puts full scale at {status.offsetDb.toFixed(1)} dB SPL and reads this calibrator as{' '}
            {target.levelDb} dB.
          </p>
        </div>
      );
  }
}
