import { formatLevel } from '../lib/format';
import { useFrameValue } from '../lib/useFrame';
import { useStore } from '../state/store';
import type { Tile } from '../state/store';
import { TIME_WEIGHTING_LABEL, WEIGHTING_LABEL, type Weighting } from '../types';

/**
 * The big number: time-weighted sound pressure level, with max, min and peak.
 *
 * The level name is built from what is actually in force — `LAF` is
 * A-weighted, Fast — because "SPL: 94.2" on its own is not a measurement anyone
 * can quote. The unit follows the calibration: **dB SPL** when calibrated,
 * **dBFS** when not, and the tile says which in a way that cannot be mistaken
 * for decoration.
 */

interface Options {
  weighting?: Weighting;
}

export function SplTile({ tile }: { tile: Tile }) {
  const frame = useFrameValue(100);
  const timeWeighting = useStore((s) => s.config.timeWeighting);
  const weighting = ((tile.options as Options).weighting ?? 'a') as Weighting;

  const spl = frame?.spl.find((s) => s.weighting === weighting);
  const calibrated = frame?.calibrated ?? false;
  const name = `L${WEIGHTING_LABEL[weighting]}${timeWeighting[0].toUpperCase()}`;

  return (
    <div className="tile-body spl">
      <div className="spl-main">
        <span className="spl-name">{name}</span>
        <span className="spl-value">{formatLevel(spl?.level)}</span>
        <span className={calibrated ? 'spl-unit' : 'spl-unit uncal'}>
          {calibrated ? 'dB SPL' : 'dBFS'}
        </span>
      </div>
      <dl className="spl-stats">
        <div>
          <dt>max</dt>
          <dd>{formatLevel(spl?.max)}</dd>
        </div>
        <div>
          <dt>min</dt>
          <dd>{formatLevel(spl?.min)}</dd>
        </div>
        <div>
          <dt>peak</dt>
          <dd>{formatLevel(spl?.peak)}</dd>
        </div>
      </dl>
      {!calibrated && (
        <p className="tile-warn">
          Not calibrated — these are full-scale levels, not sound pressure levels.
        </p>
      )}
    </div>
  );
}

export function SplSettings({ tile }: { tile: Tile }) {
  const setTileOptions = useStore((s) => s.setTileOptions);
  const setConfig = useStore((s) => s.setConfig);
  const timeWeighting = useStore((s) => s.config.timeWeighting);
  const opts = tile.options as Options;

  return (
    <>
      <label>
        Weighting
        <select
          value={opts.weighting ?? 'a'}
          onChange={(e) => setTileOptions(tile.id, { weighting: e.target.value })}
        >
          <option value="a">A</option>
          <option value="c">C</option>
          <option value="z">Z</option>
        </select>
      </label>
      <label>
        Time weighting
        <select
          value={timeWeighting}
          onChange={(e) =>
            void setConfig((c) => ({ ...c, timeWeighting: e.target.value as typeof timeWeighting }))
          }
        >
          {(['fast', 'slow', 'impulse'] as const).map((t) => (
            <option key={t} value={t}>
              {TIME_WEIGHTING_LABEL[t]}
            </option>
          ))}
        </select>
      </label>
      <p className="hint">
        Time weighting is a property of the measurement, so it applies to every SPL tile at once.
      </p>
    </>
  );
}
