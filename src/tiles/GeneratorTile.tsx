import { useStore } from '../state/store';
import type { Tile } from '../state/store';
import { expectedPeakDbfs, type Signal } from '../types';

/**
 * The signal generator.
 *
 * Two things this tile insists on, both because the output goes into a PA at
 * measurement level and mistakes there are expensive:
 *
 * - **The signal starts Off and is never restored on launch.** Opening a
 *   measurement app should not put pink noise into a rig before anyone has
 *   touched anything, so the level and shaping persist but the signal does not.
 * - **The expected peak is shown next to the level.** Level is dBFS *RMS*, and
 *   noise has a crest factor around 12 dB, so pink noise at −6 dBFS RMS clips
 *   hard while reading like a conservative setting. The tile says so before the
 *   converter finds out.
 */

const SIGNALS: { value: Signal['kind']; label: string }[] = [
  { value: 'off', label: 'Off' },
  { value: 'pink', label: 'Pink noise' },
  { value: 'white', label: 'White noise' },
  { value: 'sine', label: 'Sine' },
  { value: 'sweep', label: 'Sweep' },
];

function withKind(kind: Signal['kind'], previous: Signal): Signal {
  switch (kind) {
    case 'off':
      return { kind: 'off' };
    case 'white':
      return { kind: 'white' };
    case 'pink':
      return { kind: 'pink' };
    case 'sine':
      return { kind: 'sine', hz: previous.kind === 'sine' ? previous.hz : 1000 };
    case 'sweep':
      return previous.kind === 'sweep'
        ? previous
        : { kind: 'sweep', fromHz: 20, toHz: 20000, seconds: 2 };
  }
}

export function GeneratorTile({ tile }: { tile: Tile }) {
  void tile;
  const generator = useStore((s) => s.generator);
  const setGenerator = useStore((s) => s.setGenerator);
  const channel = useStore((s) => s.generatorChannel);
  const setChannel = useStore((s) => s.setGeneratorChannel);
  const status = useStore((s) => s.status);

  const peak = expectedPeakDbfs(generator);
  const clipping = peak > 0 && generator.signal.kind !== 'off';
  const running = generator.signal.kind !== 'off';
  const outChannels = status.output?.channels ?? 2;

  return (
    <div className="tile-body gen">
      <div className="gen-row">
        <label>
          Signal
          <select
            value={generator.signal.kind}
            onChange={(e) =>
              void setGenerator((g) => ({
                ...g,
                signal: withKind(e.target.value as Signal['kind'], g.signal),
              }))
            }
          >
            {SIGNALS.map((s) => (
              <option key={s.value} value={s.value}>
                {s.label}
              </option>
            ))}
          </select>
        </label>

        {generator.signal.kind === 'sine' && (
          <label>
            Hz
            <input
              type="number"
              min={1}
              max={20000}
              value={generator.signal.hz}
              onChange={(e) =>
                void setGenerator((g) => ({
                  ...g,
                  signal: { kind: 'sine', hz: Math.max(1, Number(e.target.value)) },
                }))
              }
            />
          </label>
        )}

        <label>
          Output
          <select
            value={channel}
            onChange={(e) => void setChannel(Number(e.target.value))}
          >
            {Array.from({ length: Math.max(1, outChannels) }, (_, i) => (
              <option key={i} value={i}>
                {i + 1}
              </option>
            ))}
          </select>
        </label>

        <button
          type="button"
          className={running ? 'primary' : ''}
          onClick={() =>
            void setGenerator((g) => ({
              ...g,
              signal: running ? { kind: 'off' } : { kind: 'pink' },
            }))
          }
        >
          {running ? 'Mute' : 'Pink'}
        </button>
      </div>

      <div className="gen-level">
        <input
          type="range"
          min={-80}
          max={0}
          step={0.5}
          value={generator.levelDbfs}
          aria-label="Output level"
          onChange={(e) =>
            void setGenerator((g) => ({ ...g, levelDbfs: Number(e.target.value) }))
          }
        />
        <span className="gen-value">{generator.levelDbfs.toFixed(1)}</span>
        <span className="gen-unit">dBFS RMS</span>
      </div>

      <p className={clipping ? 'tile-warn' : 'hint'}>
        {generator.signal.kind === 'off'
          ? 'Not generating.'
          : clipping
            ? `Peaks reach about ${peak.toFixed(1)} dBFS — this will clip. Turn it down.`
            : `Peaks reach about ${peak.toFixed(1)} dBFS.`}
      </p>

      {!status.output && status.running && (
        <p className="tile-warn">
          No output opened on this device, so nothing is being generated.
        </p>
      )}

      {status.output && !status.clockShared && (
        <p className="tile-warn">
          Output is on {status.output.device}, not the input device — two clocks. The internal
          reference will drift apart from the measurement, so find the delay again every few
          minutes, or use an interface whose input and output are one device.
        </p>
      )}
    </div>
  );
}

export function GeneratorSettings({ tile }: { tile: Tile }) {
  void tile;
  const generator = useStore((s) => s.generator);
  const setGenerator = useStore((s) => s.setGenerator);

  return (
    <>
      <label>
        High-pass (Hz)
        <input
          type="number"
          min={0}
          placeholder="none"
          value={generator.highPassHz ?? ''}
          onChange={(e) =>
            void setGenerator((g) => ({
              ...g,
              highPassHz: e.target.value ? Number(e.target.value) : null,
            }))
          }
        />
      </label>
      <label>
        Low-pass (Hz)
        <input
          type="number"
          min={0}
          placeholder="none"
          value={generator.lowPassHz ?? ''}
          onChange={(e) =>
            void setGenerator((g) => ({
              ...g,
              lowPassHz: e.target.value ? Number(e.target.value) : null,
            }))
          }
        />
      </label>
      <p className="hint">
        Band-limiting applies to the noise sources. Useful for driving a single way of a
        system without exciting the others.
      </p>
    </>
  );
}
