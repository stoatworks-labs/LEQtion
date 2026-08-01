/**
 * Typed wrappers over the Tauri commands, plus the frame bus.
 *
 * Nothing else in the app calls `invoke` or `listen` directly. That keeps every
 * command name in one file — so a rename on the Rust side has exactly one place
 * to break — and it is where the frame bus lives.
 *
 * ## Why frames do not go through React state
 *
 * Frames arrive thirty times a second and carry several hundred band levels.
 * Putting them in a `useState` re-renders the whole tree at 30 Hz, and the
 * canvas tiles do not want a re-render at all — they want the latest numbers at
 * the moment they happen to be painting. So frames land in one mutable slot and
 * subscribers are notified; a canvas reads the slot inside its own animation
 * frame, and a numeric readout throttles itself to a rate a person can read.
 */
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';

import type {
  BandPlan,
  Calibration,
  CalibrationStatus,
  CalibrationTarget,
  CaptureOptions,
  DelayEstimate,
  DeviceInfo,
  EngineConfig,
  Frame,
  GeneratorConfig,
  HistoryPoint,
  HostInfo,
  LogStatus,
  ReferenceSource,
  SeriesInfo,
  SessionStatus,
  Startup,
  TransferConfig,
  TransferPlan,
} from '../types';

const FRAME_EVENT = 'leqtion://frame';

export const api = {
  startup: () => invoke<Startup>('startup'),
  listHosts: () => invoke<HostInfo[]>('list_hosts'),
  listDevices: (host?: string) => invoke<DeviceInfo[]>('list_devices', { host: host ?? null }),
  start: (options: CaptureOptions) => invoke<SessionStatus>('start', { options }),
  stop: () => invoke<SessionStatus>('stop'),
  status: () => invoke<SessionStatus>('status'),
  frame: () => invoke<Frame>('frame'),
  bandPlan: () => invoke<BandPlan>('band_plan'),
  setConfig: (config: EngineConfig) => invoke<BandPlan>('set_config', { config }),
  resetMeasurement: () => invoke<void>('reset_measurement'),
  resetPeakHold: () => invoke<void>('reset_peak_hold'),
  beginCalibration: (target: CalibrationTarget) =>
    invoke<void>('begin_calibration', { target }),
  calibrationStatus: () => invoke<CalibrationStatus | null>('calibration_status'),
  cancelCalibration: () => invoke<void>('cancel_calibration'),
  acceptCalibration: () => invoke<Calibration>('accept_calibration'),
  clearCalibration: () => invoke<void>('clear_calibration'),
  currentCalibration: () => invoke<Calibration | null>('current_calibration'),
  saveLayout: (layout: unknown) => invoke<void>('save_layout', { layout }),

  historySeries: () => invoke<SeriesInfo[]>('history_series'),
  historyView: (id: string, seconds: number, maxPoints: number) =>
    invoke<HistoryPoint[]>('history_view', { id, seconds, maxPoints }),
  startLogging: (path?: string) => invoke<LogStatus>('start_logging', { path: path ?? null }),
  stopLogging: () => invoke<LogStatus>('stop_logging'),
  loggingStatus: () => invoke<LogStatus>('logging_status'),

  listOutputDevices: (host?: string) =>
    invoke<DeviceInfo[]>('list_output_devices', { host: host ?? null }),
  setGenerator: (config: GeneratorConfig, channel: number) =>
    invoke<void>('set_generator', { config, channel }),
  setReference: (reference: ReferenceSource) =>
    invoke<SessionStatus>('set_reference', { reference }),
  setTransferConfig: (config: TransferConfig) =>
    invoke<TransferPlan>('set_transfer_config', { config }),
  transferPlan: () => invoke<TransferPlan>('transfer_plan'),
  resetTransfer: () => invoke<void>('reset_transfer'),
  findDelay: () => invoke<DelayEstimate | null>('find_delay'),
  setDelaySamples: (samples: number) => invoke<void>('set_delay_samples', { samples }),
  impulseResponse: (maxPoints: number) =>
    invoke<number[]>('impulse_response', { maxPoints }),
};

type FrameListener = (frame: Frame) => void;

let latest: Frame | null = null;
const listeners = new Set<FrameListener>();
let unlisten: UnlistenFn | null = null;
let starting: Promise<void> | null = null;

/** The most recent frame, or null before the first one arrives. */
export function currentFrame(): Frame | null {
  return latest;
}

/**
 * Subscribe to frames. Returns an unsubscribe function.
 *
 * The Tauri listener is attached once, on the first subscription, and left
 * attached — tearing it down and rebuilding it as tiles mount and unmount would
 * drop frames every time the layout changed.
 */
export function onFrame(fn: FrameListener): () => void {
  listeners.add(fn);
  if (!unlisten && !starting) {
    starting = listen<Frame>(FRAME_EVENT, (event) => {
      latest = event.payload;
      for (const l of listeners) l(event.payload);
    }).then((fn) => {
      unlisten = fn;
      starting = null;
    });
  }
  return () => {
    listeners.delete(fn);
  };
}

/** Push a frame in by hand. Used for the first paint and by the tests. */
export function publishFrame(frame: Frame): void {
  latest = frame;
  for (const l of listeners) l(frame);
}

/** Turn an unknown thrown value into something worth showing a user. */
export function errorText(e: unknown): string {
  if (typeof e === 'string') return e;
  if (e instanceof Error) return e.message;
  return String(e);
}
