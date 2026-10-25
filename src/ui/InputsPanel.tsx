import { inputName, inputsName } from '../lib/format';
import { useFrameValue } from '../lib/useFrame';
import { useStore } from '../state/store';
import type { Routing } from '../types';
import { useInputCount } from '../lib/useInputCount';
import { InputIndexSelect, InputSelect } from './InputSelect';

/**
 * Which input every meter reads.
 *
 * One table rather than a select on every tile, because the routing belongs to
 * the measurement: two RTA tiles read the same analyser, and the LEQs keep
 * integrating whether or not a tile is showing them. The right-hand column is
 * what the engine says each meter is actually reading, which differs from the
 * setting only when the device has fewer inputs than the routing names — and
 * then it is the thing worth seeing.
 *
 * The calibration note is the point of the panel as much as the routing is: a
 * calibration belongs to the levels input, and only meters on that input are
 * dB SPL (AGENTS.md §4.4).
 */
export function InputsPanel() {
  const config = useStore((s) => s.config);
  const setConfig = useStore((s) => s.setConfig);
  const calibrated = useStore((s) => s.calibration != null);
  const channels = useInputCount();
  const inputs = useFrameValue(500)?.inputs;

  const route = (patch: Partial<Routing>) =>
    void setConfig((c) => ({ ...c, routing: { ...c.routing, ...patch } }));
  const levels = inputName(config.channel);

  const toggleLoudness = (i: number) => {
    const set = new Set(config.routing.loudness);
    if (set.has(i)) set.delete(i);
    else set.add(i);
    // Never empty: a loudness meter of no inputs would read silence and look
    // like a measurement of it.
    if (set.size === 0) return;
    route({ loudness: [...set].sort((a, b) => a - b) });
  };

  return (
    <div className="panel inputs">
      <table className="inputs-table">
        <thead>
          <tr>
            <th>Meter</th>
            <th>Input</th>
            <th>Reading</th>
          </tr>
        </thead>
        <tbody>
          <tr>
            <td>
              Levels
              <span className="inputs-what">SPL, LEQ, bargraph, history, log, calibration</span>
            </td>
            <td>
              <InputSelect
                label="Levels input"
                value={config.channel}
                onChange={(channel) => channel && void setConfig((c) => ({ ...c, channel }))}
              />
            </td>
            <td>{inputs ? inputName(inputs.levels) : '—'}</td>
          </tr>
          <tr>
            <td>
              RTA
              <span className="inputs-what">and spectrograph</span>
            </td>
            <td>
              <InputSelect
                label="RTA input"
                value={config.routing.spectrum}
                follow={`Same as the levels (${levels})`}
                onChange={(spectrum) => route({ spectrum })}
              />
            </td>
            <td>{inputs ? inputName(inputs.spectrum) : '—'}</td>
          </tr>
          <tr>
            <td>
              RTA comparison
              <span className="inputs-what">a second trace, drawn as a line</span>
            </td>
            <td>
              <InputSelect
                label="RTA comparison input"
                value={config.routing.compare}
                off
                onChange={(compare) => route({ compare })}
              />
            </td>
            <td>{inputs?.compare ? inputName(inputs.compare) : 'off'}</td>
          </tr>
          <tr>
            <td>
              Transfer function
              <span className="inputs-what">the measured side; the reference is set on the tile</span>
            </td>
            <td>
              <InputSelect
                label="Transfer function measurement input"
                value={config.routing.transfer}
                follow={`Same as the levels (${levels})`}
                onChange={(transfer) => route({ transfer })}
              />
            </td>
            <td>{inputs ? inputName(inputs.transfer) : '—'}</td>
          </tr>
          <tr>
            <td>
              Vectorscope
              <span className="inputs-what">and correlation</span>
            </td>
            <td className="inputs-pair">
              L
              <InputIndexSelect
                label="Vectorscope left input"
                value={config.routing.stereo.left}
                onChange={(left) => route({ stereo: { ...config.routing.stereo, left } })}
              />
              R
              <InputIndexSelect
                label="Vectorscope right input"
                value={config.routing.stereo.right}
                onChange={(right) => route({ stereo: { ...config.routing.stereo, right } })}
              />
            </td>
            <td>{inputs ? inputsName([inputs.stereo.left, inputs.stereo.right]) : '—'}</td>
          </tr>
          <tr>
            <td>
              Loudness
              <span className="inputs-what">LUFS, each input weighted 1.0</span>
            </td>
            <td className="inputs-chips">
              {Array.from({ length: Math.max(channels, ...config.routing.loudness.map((i) => i + 1)) }, (_, i) => (
                <label key={i} className="check chip-check">
                  <input
                    type="checkbox"
                    checked={config.routing.loudness.includes(i)}
                    onChange={() => toggleLoudness(i)}
                  />
                  {i + 1}
                </label>
              ))}
            </td>
            <td>{inputs ? inputsName(inputs.loudness) : '—'}</td>
          </tr>
        </tbody>
      </table>
      <p className="hint">
        {calibrated
          ? `The calibration belongs to the levels input (${levels}). Meters on any other input read dBFS, and say so.`
          : 'Nothing is calibrated, so every level reads dBFS.'}{' '}
        Loudness is always LUFS, relative to full scale. Surround inputs are weighted 1.0 like the
        rest, not BS.1770&rsquo;s 1.41.
      </p>
    </div>
  );
}
