import { useStore } from '../state/store';

/** Inputs on the open stream, or on the selected device before one is open. */
export function useInputCount(): number {
  return useStore((s) => {
    const device =
      s.devices.find((d) => d.name === s.selectedDevice) ?? s.devices.find((d) => d.isDefault);
    return Math.max(1, s.status.stream?.channels ?? device?.maxChannels ?? 1);
  });
}
