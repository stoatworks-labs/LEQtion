/**
 * The wire format, mirroring the Rust types in `leqtion-dsp` and `leqtion-audio`.
 *
 * Kept by hand rather than generated. The surface is small and stable, and a
 * code generator would be a build step and a dependency to maintain for the
 * sake of about a hundred lines. The rule is that every type here names the Rust
 * type it mirrors, so a change on one side has an obvious counterpart.
 */

/** `leqtion_dsp::weighting::Weighting` */
export type Weighting = 'a' | 'c' | 'z';

/** `leqtion_dsp::spl::TimeWeighting` */
export type TimeWeighting = 'fast' | 'slow' | 'impulse';

/** `leqtion_dsp::bands::Fraction` */
export type Fraction = '1/1' | '1/3' | '1/6' | '1/12' | '1/24' | '1/48';
export const FRACTIONS: Fraction[] = ['1/1', '1/3', '1/6', '1/12', '1/24', '1/48'];

/** `leqtion_dsp::window::WindowKind` */
export type WindowKind = 'hann' | 'hamming' | 'blackman-harris' | 'flat-top' | 'rectangular';
export const WINDOWS: { value: WindowKind; label: string }[] = [
  { value: 'hann', label: 'Hann' },
  { value: 'hamming', label: 'Hamming' },
  { value: 'blackman-harris', label: 'Blackman-Harris' },
  { value: 'flat-top', label: 'Flat-top' },
  { value: 'rectangular', label: 'Rectangular' },
];

/** `leqtion_dsp::spectrum::Averaging` */
export type Averaging = 'fast' | 'slow' | 'long' | 'infinite';

export const FFT_SIZES = [2048, 4096, 8192, 16384, 32768, 65536] as const;
export type FftSize = (typeof FFT_SIZES)[number];

/** `leqtion_dsp::spectrum::SpectrumConfig` */
export interface SpectrumConfig {
  fraction: Fraction;
  fftSize: FftSize;
  window: WindowKind;
  hopFraction: number;
  averaging: Averaging;
  peakHold: boolean;
}

/** `leqtion_dsp::engine::ChannelSelect` */
export type ChannelSelect = { kind: 'channel'; index: number } | { kind: 'mix' };

/** `leqtion_dsp::leq::LeqWindow` */
export type LeqWindow = { kind: 'sliding'; seconds: number } | { kind: 'elapsed' };

/** `leqtion_dsp::leq::LeqSpec` */
export interface LeqSpec {
  id: string;
  label: string;
  weighting: Weighting;
  window: LeqWindow;
}

/** `leqtion_dsp::engine::EngineConfig` */
export interface EngineConfig {
  spectrum: SpectrumConfig;
  timeWeighting: TimeWeighting;
  channel: ChannelSelect;
  leqs: LeqSpec[];
}

/** `leqtion_dsp::bands::Band` */
export interface Band {
  k: number;
  fc: number;
  flo: number;
  fhi: number;
  label: string;
  binLo: number;
  binHi: number;
}

/** `leqtion_dsp::bands::BandPlan` */
export interface BandPlan {
  fraction: Fraction;
  fftSize: number;
  sampleRate: number;
  enbw: number;
  binHz: number;
  bands: Band[];
  /** Below this the display is interpolated rather than measured. */
  resolvedAboveHz: number;
}

/** `leqtion_dsp::engine::SplReading` */
export interface SplReading {
  weighting: Weighting;
  level: number;
  max: number;
  min: number;
  peak: number;
}

/** `leqtion_dsp::leq::LeqReading` */
export interface LeqReading {
  id: string;
  label: string;
  weighting: Weighting;
  value: number;
  calibrated: boolean;
  elapsedSeconds: number;
  integratedSeconds: number;
  fill: number;
}

/** `leqtion_dsp::calibration::CalibrationStatus` */
export type CalibrationStatus =
  | { state: 'settling'; progress: number }
  | { state: 'unstable'; spreadDb: number }
  | { state: 'wrongFrequency'; measuredHz: number; expectedHz: number }
  | { state: 'clipping' }
  | { state: 'tooQuiet'; levelDbfs: number }
  | { state: 'ready'; measuredDbfs: number; spreadDb: number; offsetDb: number };

/** `leqtion_dsp::calibration::CalibrationTarget` */
export interface CalibrationTarget {
  levelDb: number;
  frequencyHz: number;
}

/** `leqtion_dsp::calibration::Calibration` */
export interface Calibration {
  offsetDb: number;
  target: CalibrationTarget;
  measuredDbfs: number;
  device: string;
  channel: number;
  takenAt: string;
}

/** `leqtion_dsp::engine::Frame` */
export interface Frame {
  sampleRate: number;
  planRevision: number;
  calibrated: boolean;
  bandsDb: number[];
  peaksDb: number[];
  spl: SplReading[];
  leqs: LeqReading[];
  timeWeighting: TimeWeighting;
  dominantHz: number | null;
  inputPeakDbfs: number;
  clipped: boolean;
  elapsedSeconds: number;
  calibration?: CalibrationStatus;
}

/** `leqtion_audio::HostInfo` */
export interface HostInfo {
  id: string;
  name: string;
  available: boolean;
  isDefault: boolean;
  note?: string;
}

/** `leqtion_audio::DeviceInfo` */
export interface DeviceInfo {
  host: string;
  name: string;
  maxChannels: number;
  sampleRates: number[];
  defaultSampleRate: number;
  isDefault: boolean;
}

/** `leqtion_audio::StreamInfo` */
export interface StreamInfo {
  host: string;
  device: string;
  channels: number;
  sampleRate: number;
  sampleFormat: string;
}

/** `leqtion::session::SessionStatus` */
export interface SessionStatus {
  running: boolean;
  stream?: StreamInfo;
  droppedFrames: number;
  streamErrors: number;
}

/** `leqtion_audio::CaptureOptions` */
export interface CaptureOptions {
  host?: string | null;
  device?: string | null;
  sampleRate?: number | null;
  bufferFrames?: number | null;
}

export interface Settings {
  engine: EngineConfig;
  host: string | null;
  device: string | null;
  sampleRate: number | null;
  calibrations: Calibration[];
  layout: unknown;
}

/** `leqtion::Startup` */
export interface Startup {
  settings: Settings;
  hosts: HostInfo[];
  devices: DeviceInfo[];
  status: SessionStatus;
  plan: BandPlan;
  calibrationTargets: CalibrationTarget[];
  version: string;
}

export const WEIGHTING_LABEL: Record<Weighting, string> = { a: 'A', c: 'C', z: 'Z' };
export const TIME_WEIGHTING_LABEL: Record<TimeWeighting, string> = {
  fast: 'Fast',
  slow: 'Slow',
  impulse: 'Impulse',
};

export const DEFAULT_ENGINE_CONFIG: EngineConfig = {
  spectrum: {
    fraction: '1/12',
    fftSize: 16384,
    window: 'hann',
    hopFraction: 0.5,
    averaging: 'slow',
    peakHold: false,
  },
  timeWeighting: 'fast',
  channel: { kind: 'channel', index: 0 },
  leqs: [],
};
