import { useFrameValue } from '../lib/useFrame';
import { useStore } from '../state/store';

/**
 * Input selection and the state of the measurement chain.
 *
 * Three things are surfaced here that a meter must never hide:
 *
 * - **whether it is calibrated**, and to what;
 * - **the sample rate**, because the weighting filters are more accurate at
 *   96 kHz than at 44.1 (see `leqtion-dsp`'s `weighting` module);
 * - **dropped frames**, because a dropped buffer means time went missing and
 *   every LEQ on screen is wrong by an unknown amount. That one is stated
 *   loudly rather than logged.
 */
export function DeviceBar({ onCalibrate }: { onCalibrate: () => void }) {
  const hosts = useStore((s) => s.hosts);
  const devices = useStore((s) => s.devices);
  const status = useStore((s) => s.status);
  const selectedHost = useStore((s) => s.selectedHost);
  const selectedDevice = useStore((s) => s.selectedDevice);
  const selectedRate = useStore((s) => s.selectedRate);
  const selectHost = useStore((s) => s.selectHost);
  const selectDevice = useStore((s) => s.selectDevice);
  const selectRate = useStore((s) => s.selectRate);
  const start = useStore((s) => s.start);
  const stop = useStore((s) => s.stop);
  const config = useStore((s) => s.config);
  const setConfig = useStore((s) => s.setConfig);
  const calibration = useStore((s) => s.calibration);
  const frame = useFrameValue(500);

  const device = devices.find((d) => d.name === selectedDevice) ?? devices.find((d) => d.isDefault);
  const rates = device?.sampleRates ?? [];
  const channels = status.stream?.channels ?? device?.maxChannels ?? 1;
  const host = hosts.find((h) => h.id === selectedHost);

  return (
    <div className="devicebar">
      <label>
        Backend
        <select
          value={selectedHost ?? hosts.find((h) => h.isDefault)?.id ?? ''}
          onChange={(e) => void selectHost(e.target.value || null)}
          disabled={status.running}
        >
          {hosts.map((h) => (
            <option key={h.id} value={h.id} disabled={!h.available}>
              {h.name}
              {h.available ? '' : ' (not in this build)'}
            </option>
          ))}
        </select>
      </label>

      <label className="grow">
        Input
        <select
          value={selectedDevice ?? device?.name ?? ''}
          onChange={(e) => selectDevice(e.target.value || null)}
          disabled={status.running}
        >
          {devices.length === 0 && <option value="">no inputs found</option>}
          {devices.map((d) => (
            <option key={d.name} value={d.name}>
              {d.name}
              {d.isDefault ? ' (default)' : ''}
            </option>
          ))}
        </select>
      </label>

      <label>
        Rate
        <select
          value={selectedRate ?? device?.defaultSampleRate ?? ''}
          onChange={(e) => selectRate(e.target.value ? Number(e.target.value) : null)}
          disabled={status.running}
        >
          {rates.map((r) => (
            <option key={r} value={r}>
              {r / 1000} kHz
            </option>
          ))}
        </select>
      </label>

      <label>
        Channel
        <select
          value={config.channel.kind === 'mix' ? 'mix' : String(config.channel.index)}
          onChange={(e) =>
            void setConfig((c) => ({
              ...c,
              channel:
                e.target.value === 'mix'
                  ? { kind: 'mix' }
                  : { kind: 'channel', index: Number(e.target.value) },
            }))
          }
        >
          {Array.from({ length: Math.max(1, channels) }, (_, i) => (
            <option key={i} value={String(i)}>
              {i + 1}
            </option>
          ))}
          {channels > 1 && <option value="mix">mix</option>}
        </select>
      </label>

      {status.running ? (
        <button type="button" onClick={() => void stop()}>
          Stop
        </button>
      ) : (
        <button type="button" className="primary" onClick={() => void start()} disabled={devices.length === 0}>
          Start
        </button>
      )}

      <button type="button" onClick={onCalibrate} disabled={!status.running}>
        Calibrate…
      </button>

      <span className="spacer" />

      {host && !host.available && host.note && <span className="chip warn">{host.note}</span>}

      {status.running && status.stream && (
        <span className="chip">
          {status.stream.sampleRate / 1000} kHz · {status.stream.sampleFormat}
        </span>
      )}

      {calibration ? (
        <span className="chip good" title={`Taken ${calibration.takenAt || 'at an unknown time'} on ${calibration.device}`}>
          calibrated · full scale {calibration.offsetDb.toFixed(1)} dB SPL
        </span>
      ) : (
        <span className="chip warn">not calibrated — levels are dBFS</span>
      )}

      {frame?.clipped && <span className="chip bad">input clipped</span>}

      {status.droppedFrames > 0 && (
        <span className="chip bad" title="The analysis thread could not keep up, so audio was discarded. Any LEQ covering that period is short by an unknown amount.">
          {status.droppedFrames} frames dropped — restart the measurement
        </span>
      )}
    </div>
  );
}
