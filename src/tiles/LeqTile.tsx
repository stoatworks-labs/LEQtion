import { describeWindow, formatElapsed, formatLevel, levelName } from '../lib/format';
import { useFrameValue } from '../lib/useFrame';
import { useStore } from '../state/store';
import type { Tile } from '../state/store';

/**
 * One user-defined LEQ.
 *
 * The tile points at an LEQ *by id* rather than carrying its own settings. The
 * accumulator lives in the engine, so a sliding five-minute LAeq keeps
 * integrating whether or not a tile happens to be showing it, and two tiles can
 * show the same LEQ without measuring it twice.
 *
 * A sliding window shows how full it is. This matters more than it looks: an
 * LAeq,5min two minutes into a measurement is a real number over two minutes, not
 * a five-minute figure, and quoting it as one would be wrong. The fill bar and
 * the "filling" note say so until the window is complete.
 */

interface Options {
  leqId?: string;
}

export function LeqTile({ tile }: { tile: Tile }) {
  const frame = useFrameValue(200);
  const leqs = useStore((s) => s.config.leqs);
  const id = (tile.options as Options).leqId ?? leqs[0]?.id;

  const spec = leqs.find((l) => l.id === id);
  const reading = frame?.leqs.find((l) => l.id === id);

  if (!spec) {
    return (
      <div className="tile-body leq">
        <p className="tile-empty">
          No LEQ selected. Open this tile&rsquo;s settings to pick one, or add one in the toolbar.
        </p>
      </div>
    );
  }

  const filling = spec.window.kind === 'sliding' && (reading?.fill ?? 0) < 0.999;

  return (
    <div className="tile-body leq">
      <div className="leq-head">
        <span className="leq-name">{reading?.label ?? spec.label ?? spec.id}</span>
        <span className="leq-window">{describeWindow(spec.window)}</span>
      </div>
      <div className="leq-main">
        <span className="leq-value">{formatLevel(reading?.value)}</span>
        <span className={reading?.calibrated ? 'spl-unit' : 'spl-unit uncal'}>
          {reading?.calibrated ? 'dB SPL' : 'dBFS'}
        </span>
      </div>
      {spec.window.kind === 'sliding' && (
        <div className="leq-fill" title={`${Math.round((reading?.fill ?? 0) * 100)}% of the window`}>
          <span style={{ width: `${Math.round((reading?.fill ?? 0) * 100)}%` }} />
        </div>
      )}
      <div className="leq-foot">
        <span>{formatElapsed(reading?.elapsedSeconds ?? 0)}</span>
        {filling && <span className="leq-filling">filling</span>}
      </div>
    </div>
  );
}

export function LeqSettings({ tile }: { tile: Tile }) {
  const setTileOptions = useStore((s) => s.setTileOptions);
  const leqs = useStore((s) => s.config.leqs);
  const updateLeq = useStore((s) => s.updateLeq);
  const id = (tile.options as Options).leqId ?? leqs[0]?.id;
  const spec = leqs.find((l) => l.id === id);

  return (
    <>
      <label>
        Show
        <select value={id ?? ''} onChange={(e) => setTileOptions(tile.id, { leqId: e.target.value })}>
          {leqs.length === 0 && <option value="">no LEQs defined</option>}
          {leqs.map((l) => (
            <option key={l.id} value={l.id}>
              {l.label || levelName(l.weighting, l.window)}
            </option>
          ))}
        </select>
      </label>
      {spec && (
        <>
          <label>
            Name
            <input
              type="text"
              placeholder={levelName(spec.weighting, spec.window)}
              value={spec.label}
              onChange={(e) => void updateLeq(spec.id, { label: e.target.value })}
            />
          </label>
          <p className="hint">
            Weighting and window are edited in the toolbar&rsquo;s LEQ list, because they belong to
            the measurement rather than to this tile.
          </p>
        </>
      )}
    </>
  );
}

