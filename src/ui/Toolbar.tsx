import { useEffect, useState } from 'react';

import { api, errorText } from '../lib/ipc';
import { useStore } from '../state/store';
import { TILE_TYPES } from '../tiles/registry';
import { levelName } from '../lib/format';
import { FFT_SIZES, FRACTIONS, WINDOWS, type Averaging, type Fraction, type LeqWindow, type LogStatus } from '../types';

/** Window lengths offered when adding an LEQ, in seconds. */
const LEQ_PRESETS: { label: string; window: LeqWindow }[] = [
  { label: '1 s', window: { kind: 'sliding', seconds: 1 } },
  { label: '5 s', window: { kind: 'sliding', seconds: 5 } },
  { label: '1 min', window: { kind: 'sliding', seconds: 60 } },
  { label: '5 min', window: { kind: 'sliding', seconds: 300 } },
  { label: '15 min', window: { kind: 'sliding', seconds: 900 } },
  { label: '1 h', window: { kind: 'sliding', seconds: 3600 } },
  { label: 'since reset', window: { kind: 'elapsed' } },
];

const AVERAGING: { value: Averaging; label: string }[] = [
  { value: 'fast', label: 'Fast' },
  { value: 'slow', label: 'Slow' },
  { value: 'long', label: 'Long' },
  { value: 'infinite', label: 'Infinite' },
];

export function Toolbar() {
  const config = useStore((s) => s.config);
  const setConfig = useStore((s) => s.setConfig);
  const addTile = useStore((s) => s.addTile);
  const resetLayout = useStore((s) => s.resetLayout);
  const plan = useStore((s) => s.plan);
  const [panel, setPanel] = useState<'none' | 'analysis' | 'leqs' | 'tiles'>('none');

  const toggle = (p: typeof panel) => setPanel(panel === p ? 'none' : p);

  return (
    <div className="toolbar">
      <div className="toolbar-row">
        <button type="button" onClick={() => toggle('analysis')} aria-pressed={panel === 'analysis'}>
          Analysis
        </button>
        <button type="button" onClick={() => toggle('leqs')} aria-pressed={panel === 'leqs'}>
          LEQs <span className="badge">{config.leqs.length}</span>
        </button>
        <button type="button" onClick={() => toggle('tiles')} aria-pressed={panel === 'tiles'}>
          Add tile
        </button>
        <span className="spacer" />
        <ResetButtons />
      </div>

      {panel === 'analysis' && (
        <div className="panel">
          <label>
            Resolution
            <select
              value={config.spectrum.fraction}
              onChange={(e) =>
                void setConfig((c) => ({
                  ...c,
                  spectrum: { ...c.spectrum, fraction: e.target.value as Fraction },
                }))
              }
            >
              {FRACTIONS.map((f) => (
                <option key={f} value={f}>
                  {f} octave
                </option>
              ))}
            </select>
          </label>

          <label>
            Transform
            <select
              value={config.spectrum.fftSize}
              onChange={(e) =>
                void setConfig((c) => ({
                  ...c,
                  spectrum: { ...c.spectrum, fftSize: Number(e.target.value) as never },
                }))
              }
            >
              {FFT_SIZES.map((n) => (
                <option key={n} value={n}>
                  {n}
                </option>
              ))}
            </select>
          </label>

          <label>
            Window
            <select
              value={config.spectrum.window}
              onChange={(e) =>
                void setConfig((c) => ({
                  ...c,
                  spectrum: { ...c.spectrum, window: e.target.value as never },
                }))
              }
            >
              {WINDOWS.map((w) => (
                <option key={w.value} value={w.value}>
                  {w.label}
                </option>
              ))}
            </select>
          </label>

          <label>
            Overlap
            <select
              value={config.spectrum.hopFraction}
              onChange={(e) =>
                void setConfig((c) => ({
                  ...c,
                  spectrum: { ...c.spectrum, hopFraction: Number(e.target.value) },
                }))
              }
            >
              <option value={1}>none</option>
              <option value={0.5}>50%</option>
              <option value={0.25}>75%</option>
            </select>
          </label>

          <label>
            Averaging
            <select
              value={config.spectrum.averaging}
              onChange={(e) =>
                void setConfig((c) => ({
                  ...c,
                  spectrum: { ...c.spectrum, averaging: e.target.value as Averaging },
                }))
              }
            >
              {AVERAGING.map((a) => (
                <option key={a.value} value={a.value}>
                  {a.label}
                </option>
              ))}
            </select>
          </label>

          <label className="check">
            <input
              type="checkbox"
              checked={config.spectrum.peakHold}
              onChange={(e) =>
                void setConfig((c) => ({
                  ...c,
                  spectrum: { ...c.spectrum, peakHold: e.target.checked },
                }))
              }
            />
            Peak hold
          </label>

          {plan && (
            <p className="hint">
              {plan.bands.length} bands · {plan.binHz.toFixed(2)} Hz bins ·{' '}
              {Number.isFinite(plan.resolvedAboveHz)
                ? `measured above ${Math.round(plan.resolvedAboveHz)} Hz, interpolated below`
                : 'entirely interpolated at this transform size'}
            </p>
          )}
        </div>
      )}

      {panel === 'leqs' && <LeqPanel />}

      {panel === 'tiles' && (
        <div className="panel tiles">
          {TILE_TYPES.map((t) => (
            <button key={t.kind} type="button" className="tile-add" onClick={() => addTile(t.kind)}>
              <strong>{t.title}</strong>
              <span>{t.blurb}</span>
            </button>
          ))}
          <button type="button" className="tile-add reset" onClick={resetLayout}>
            <strong>Reset layout</strong>
            <span>Back to the default arrangement.</span>
          </button>
        </div>
      )}
    </div>
  );
}

/**
 * Start and stop the CSV log.
 *
 * In the toolbar rather than on a tile, because the log is a property of the
 * measurement and not of anything on screen: closing the chart must not stop
 * the recording, and a log that only ran while a particular tile was open would
 * have holes in it that nothing explained.
 *
 * The row count is shown because it is the only honest confirmation that
 * anything is being written — a path on its own says a file was opened, not
 * that a measurement is going into it.
 */
function LoggingButton() {
  const [log, setLog] = useState<LogStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const running = useStore((s) => s.status.running);

  useEffect(() => {
    let live = true;
    const poll = async () => {
      try {
        const s = await api.loggingStatus();
        if (live) setLog(s);
      } catch {
        /* the session may not be up yet; the next poll will find it */
      }
    };
    void poll();
    const timer = setInterval(() => void poll(), 1000);
    return () => {
      live = false;
      clearInterval(timer);
    };
  }, []);

  async function toggle() {
    setError(null);
    try {
      setLog(log?.running ? await api.stopLogging() : await api.startLogging());
    } catch (e) {
      setError(errorText(e));
    }
  }

  return (
    <>
      <button
        type="button"
        className={log?.running ? 'logging' : undefined}
        disabled={!running && !log?.running}
        onClick={() => void toggle()}
        title={log?.path ?? 'Write the measurement to a CSV, one row per interval'}
      >
        {log?.running ? `Stop logging · ${log.rows} rows` : 'Start logging'}
      </button>
      {error && <span className="chip bad">{error}</span>}
    </>
  );
}

function ResetButtons() {
  const resetMeasurement = useStore((s) => s.resetMeasurement);
  const resetPeakHold = useStore((s) => s.resetPeakHold);
  return (
    <>
      <LoggingButton />
      <button type="button" onClick={() => void resetPeakHold()}>
        Clear peaks
      </button>
      <button type="button" className="primary" onClick={() => void resetMeasurement()}>
        Reset measurement
      </button>
    </>
  );
}

/**
 * Keeps a hand-typed window length selectable.
 *
 * Without this, an LEQ created with a custom window would show whichever preset
 * the browser fell back to, and changing any other field would silently snap it
 * to that preset — losing the setting the user actually asked for.
 */
function CustomWindowOption({ window }: { window: LeqWindow }) {
  if (window.kind !== 'sliding') return null;
  const seconds = window.seconds;
  const isPreset = LEQ_PRESETS.some(
    (p) => p.window.kind === 'sliding' && p.window.seconds === seconds,
  );
  if (isPreset) return null;
  return <option value={String(seconds)}>{seconds} s</option>;
}

function LeqPanel() {
  const leqs = useStore((s) => s.config.leqs);
  const addLeq = useStore((s) => s.addLeq);
  const updateLeq = useStore((s) => s.updateLeq);
  const removeLeq = useStore((s) => s.removeLeq);
  const [custom, setCustom] = useState('');

  return (
    <div className="panel leqs">
      <table className="leq-table">
        <thead>
          <tr>
            <th>Name</th>
            <th>Weighting</th>
            <th>Window</th>
            <th />
          </tr>
        </thead>
        <tbody>
          {leqs.map((l) => (
            <tr key={l.id}>
              <td>
                <input
                  type="text"
                  value={l.label}
                  placeholder={levelName(l.weighting, l.window)}
                  onChange={(e) => void updateLeq(l.id, { label: e.target.value })}
                />
              </td>
              <td>
                <select
                  value={l.weighting}
                  onChange={(e) => void updateLeq(l.id, { weighting: e.target.value as never })}
                >
                  <option value="a">A</option>
                  <option value="c">C</option>
                  <option value="z">Z</option>
                </select>
              </td>
              <td>
                <select
                  value={l.window.kind === 'elapsed' ? 'elapsed' : String(l.window.seconds)}
                  onChange={(e) => {
                    const v = e.target.value;
                    void updateLeq(l.id, {
                      window:
                        v === 'elapsed'
                          ? { kind: 'elapsed' }
                          : { kind: 'sliding', seconds: Number(v) },
                    });
                  }}
                >
                  {LEQ_PRESETS.map((p) => (
                    <option
                      key={p.label}
                      value={p.window.kind === 'elapsed' ? 'elapsed' : String(p.window.seconds)}
                    >
                      {p.label}
                    </option>
                  ))}
                  <CustomWindowOption window={l.window} />
                </select>
              </td>
              <td>
                <button
                  type="button"
                  className="icon"
                  aria-label={`Remove ${l.label || l.id}`}
                  onClick={() => void removeLeq(l.id)}
                >
                  ✕
                </button>
              </td>
            </tr>
          ))}
          {leqs.length === 0 && (
            <tr>
              <td colSpan={4} className="hint">
                No LEQs defined. Add one and put an LEQ tile on the grid to see it.
              </td>
            </tr>
          )}
        </tbody>
      </table>

      <div className="leq-add">
        <button
          type="button"
          onClick={() =>
            void addLeq({ label: '', weighting: 'a', window: { kind: 'sliding', seconds: 60 } })
          }
        >
          Add LAeq,1min
        </button>
        <button
          type="button"
          onClick={() => void addLeq({ label: '', weighting: 'a', window: { kind: 'elapsed' } })}
        >
          Add LAeq since reset
        </button>
        <label className="custom">
          Custom window (s)
          <input
            type="number"
            min={0.05}
            step={1}
            value={custom}
            onChange={(e) => setCustom(e.target.value)}
          />
          <button
            type="button"
            disabled={!(Number(custom) > 0)}
            onClick={() => {
              void addLeq({
                label: '',
                weighting: 'a',
                window: { kind: 'sliding', seconds: Number(custom) },
              });
              setCustom('');
            }}
          >
            Add
          </button>
        </label>
      </div>
    </div>
  );
}
