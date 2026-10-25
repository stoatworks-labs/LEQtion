import { useInputCount } from '../lib/useInputCount';
import type { ChannelSelect } from '../types';

/**
 * A select for one meter's input.
 *
 * `follow` adds "same as the levels" — the `null` that single-input meters use
 * so a one-microphone rig changes one control and everything moves with it.
 * `off` adds an Off choice, for the RTA's comparison trace. Without either, the
 * value is never null.
 *
 * The inputs listed come from the open stream, or the selected device before
 * one is open, counted from one as the device bar counts them.
 */
export function InputSelect({
  value,
  onChange,
  follow,
  off,
  label,
}: {
  value: ChannelSelect | null;
  onChange: (v: ChannelSelect | null) => void;
  follow?: string;
  off?: boolean;
  label?: string;
}) {
  const channels = useInputCount();
  const encoded =
    value == null ? (off ? 'off' : 'follow') : value.kind === 'mix' ? 'mix' : `ch:${value.index}`;

  return (
    <select
      aria-label={label}
      value={encoded}
      onChange={(e) => {
        const v = e.target.value;
        onChange(
          v === 'follow' || v === 'off'
            ? null
            : v === 'mix'
              ? { kind: 'mix' }
              : { kind: 'channel', index: Number(v.slice(3)) },
        );
      }}
    >
      {follow && <option value="follow">{follow}</option>}
      {off && <option value="off">Off</option>}
      {Array.from({ length: channels }, (_, i) => (
        <option key={i} value={`ch:${i}`}>
          Input {i + 1}
        </option>
      ))}
      {/* A routed input the device no longer has stays selectable, so looking
          at the setting does not silently change it. The engine falls back to
          the last input and the tile says which it is actually reading. */}
      {value?.kind === 'channel' && value.index >= channels && (
        <option value={`ch:${value.index}`}>Input {value.index + 1} (not on this device)</option>
      )}
      {channels > 1 && <option value="mix">Mix of all</option>}
    </select>
  );
}

/** A select for one input by index, never null. */
export function InputIndexSelect({
  value,
  onChange,
  label,
}: {
  value: number;
  onChange: (v: number) => void;
  label?: string;
}) {
  const channels = useInputCount();
  return (
    <select aria-label={label} value={value} onChange={(e) => onChange(Number(e.target.value))}>
      {Array.from({ length: Math.max(channels, value + 1) }, (_, i) => (
        <option key={i} value={i}>
          Input {i + 1}
          {i >= channels ? ' (not on this device)' : ''}
        </option>
      ))}
    </select>
  );
}
