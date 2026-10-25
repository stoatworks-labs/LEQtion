import { formatElapsed, formatLevel, inputsName } from '../lib/format';
import { dbToUnit } from '../lib/plot';
import { useFrameValue } from '../lib/useFrame';
import { useStore } from '../state/store';
import type { Tile } from '../state/store';

/**
 * Programme loudness, ITU-R BS.1770 as EBU R 128 meters it.
 *
 * Every number is the engine's (`leqtion-dsp::loudness`). Two things the tile
 * has to say, because they are the two ways this reading gets misread:
 *
 * - **LUFS is relative to full scale and is never calibrated.** It is not the
 *   app's dBFS either: a full-scale sine reads 0 dBFS on the levels and
 *   −3.01 LUFS here, and both are right. The unit is on every number.
 * - **The loudness range is not stable for the first minute** (EBU Tech 3341
 *   §2.4), so it says so until a minute has been measured.
 *
 * The bars are a picture of momentary and short-term against a target line,
 * on a fixed −60 to 0 LUFS scale. The target is a display setting, not part
 * of the measurement.
 */

interface Options {
  /** LUFS. Null for no target line. */
  target?: number | null;
}

const TARGETS: { value: number | null; label: string }[] = [
  { value: null, label: 'None' },
  { value: -23, label: '−23 LUFS (EBU R 128)' },
  { value: -24, label: '−24 LKFS (ATSC A/85)' },
  { value: -16, label: '−16 LUFS' },
  { value: -14, label: '−14 LUFS' },
];

const SCALE_TOP = 0;
const SCALE_BOTTOM = -60;
/** Tech 3341 §2.4: show LRA as not yet stable for the first 60 s. */
const RANGE_SETTLES_SECONDS = 60;

export function LoudnessTile({ tile }: { tile: Tile }) {
  const frame = useFrameValue(100);
  const opts = tile.options as Options;
  const target = opts.target === undefined ? -23 : opts.target;
  const l = frame?.loudness;
  const inputs = frame?.inputs.loudness;
  const settling = (l?.elapsedSeconds ?? 0) < RANGE_SETTLES_SECONDS;

  return (
    <div className="tile-body loud">
      <div className="loud-main">
        <Readout name="M" title="Momentary, 400 ms" value={l?.momentary} unit="LUFS" />
        <Readout name="S" title="Short-term, 3 s" value={l?.shortTerm} unit="LUFS" />
        <Readout name="I" title="Integrated, gated, since reset" value={l?.integrated} unit="LUFS" big />
      </div>

      <div className="loud-bars">
        <Bar label="M" value={l?.momentary} target={target} />
        <Bar label="S" value={l?.shortTerm} target={target} />
      </div>

      <dl className="loud-stats">
        <div title="Loudness range, EBU Tech 3342">
          <dt>LRA</dt>
          <dd>
            {formatLevel(l?.range)} <span className="loud-unit">LU</span>
            {l?.range != null && settling && <span className="loud-settling"> settling</span>}
          </dd>
        </div>
        <div title="True peak since reset, 4× oversampled (BS.1770 Annex 2)">
          <dt>TP</dt>
          <dd>
            {formatLevel(l?.truePeak)} <span className="loud-unit">dBTP</span>
          </dd>
        </div>
        <div title="Highest momentary since reset">
          <dt>Max M</dt>
          <dd>{formatLevel(l?.maxMomentary)}</dd>
        </div>
        <div title="Highest short-term since reset">
          <dt>Max S</dt>
          <dd>{formatLevel(l?.maxShortTerm)}</dd>
        </div>
      </dl>

      <div className="loud-foot">
        <span>{inputs ? inputsName(inputs) : ''}</span>
        <span>{formatElapsed(l?.elapsedSeconds ?? 0)}</span>
        <span className="loud-note">relative to full scale, never calibrated</span>
      </div>
    </div>
  );
}

function Readout({
  name,
  title,
  value,
  unit,
  big,
}: {
  name: string;
  title: string;
  value: number | null | undefined;
  unit: string;
  big?: boolean;
}) {
  return (
    <div className={big ? 'loud-readout big' : 'loud-readout'} title={title}>
      <span className="loud-name">{name}</span>
      <span className="loud-value">{formatLevel(value)}</span>
      <span className="loud-unit">{unit}</span>
    </div>
  );
}

function Bar({
  label,
  value,
  target,
}: {
  label: string;
  value: number | null | undefined;
  target: number | null;
}) {
  // dbToUnit puts the top of the scale at 0; a bar grows from the left.
  const fill = value == null ? 0 : 1 - dbToUnit(value, SCALE_TOP, SCALE_BOTTOM);
  const mark = target == null ? null : 1 - dbToUnit(target, SCALE_TOP, SCALE_BOTTOM);
  return (
    <div className="loud-bar">
      <span className="loud-bar-label">{label}</span>
      <div className="loud-bar-track">
        <span className="loud-bar-fill" style={{ width: `${fill * 100}%` }} />
        {mark != null && <span className="loud-bar-target" style={{ left: `${mark * 100}%` }} />}
      </div>
    </div>
  );
}

export function LoudnessSettings({ tile }: { tile: Tile }) {
  const setTileOptions = useStore((s) => s.setTileOptions);
  const opts = tile.options as Options;
  const target = opts.target === undefined ? -23 : opts.target;
  return (
    <>
      <label>
        Target line
        <select
          value={target == null ? 'none' : String(target)}
          onChange={(e) =>
            setTileOptions(tile.id, {
              target: e.target.value === 'none' ? null : Number(e.target.value),
            })
          }
        >
          {TARGETS.map((t) => (
            <option key={t.label} value={t.value == null ? 'none' : String(t.value)}>
              {t.label}
            </option>
          ))}
        </select>
      </label>
      <p className="hint">
        The inputs are chosen under Inputs in the toolbar. Reset measurement starts the integrated
        loudness, the range, the maxima and the true peak again.
      </p>
    </>
  );
}
