/**
 * Application state: what is being measured, what is on screen, and where.
 *
 * Frames are *not* in here — see `lib/ipc.ts` for why. This store holds the
 * things that change when a person does something, which is the rate React is
 * good at.
 *
 * Configuration is deliberately one-way: a change is sent to Rust, and Rust
 * returns the band plan that resulted. The UI never computes a band table of its
 * own, so the axis labels on a chart cannot drift out of step with the data
 * plotted against them.
 */
import { create } from 'zustand';

import { api, errorText } from '../lib/ipc';
import {
  DEFAULT_ENGINE_CONFIG,
  type BandPlan,
  type Calibration,
  type CalibrationTarget,
  type DeviceInfo,
  type EngineConfig,
  type HostInfo,
  type LeqSpec,
  type GeneratorConfig,
  type ReferenceSource,
  type SessionStatus,
  type TransferConfig,
  type TransferPlan,
  DEFAULT_GENERATOR,
  DEFAULT_TRANSFER,
} from '../types';

export type TileKind =
  | 'rta'
  | 'spectrograph'
  | 'bargraph'
  | 'spl'
  | 'leq'
  | 'transfer'
  | 'generator';

export interface Tile {
  id: string;
  kind: TileKind;
  /** Grid position in columns and rows, both zero-based. */
  x: number;
  y: number;
  w: number;
  h: number;
  /** Per-tile display settings. Shape depends on `kind`; see the tile itself. */
  options: Record<string, unknown>;
}

export interface Layout {
  cols: number;
  tiles: Tile[];
}

export const GRID_COLS = 12;
/** Height of one grid row, in CSS pixels. */
export const ROW_HEIGHT = 88;
export const GRID_GAP = 10;

let nextId = 1;
export function newTileId(kind: TileKind): string {
  return `${kind}-${Date.now().toString(36)}-${nextId++}`;
}

/** Minimum useful size for each kind, in grid cells. */
export const MIN_SIZE: Record<TileKind, { w: number; h: number }> = {
  rta: { w: 4, h: 3 },
  spectrograph: { w: 4, h: 2 },
  bargraph: { w: 1, h: 3 },
  spl: { w: 2, h: 2 },
  leq: { w: 2, h: 1 },
  transfer: { w: 4, h: 3 },
  generator: { w: 3, h: 2 },
};

export const DEFAULT_SIZE: Record<TileKind, { w: number; h: number }> = {
  rta: { w: 8, h: 5 },
  spectrograph: { w: 8, h: 4 },
  bargraph: { w: 2, h: 5 },
  spl: { w: 4, h: 3 },
  leq: { w: 2, h: 2 },
  transfer: { w: 8, h: 5 },
  generator: { w: 4, h: 2 },
};

/**
 * What a first run looks like.
 *
 * An RTA and a broadband SPL, because those are the two things someone opens a
 * meter to see; one A-weighted LEQ over five minutes, because that is the
 * commonest limit in a licence; and a bargraph, because a level meter with no
 * headroom indication is how a measurement gets clipped without anyone noticing.
 */
export function defaultLayout(): Layout {
  return {
    cols: GRID_COLS,
    tiles: [
      { id: 'rta-1', kind: 'rta', x: 0, y: 0, w: 8, h: 5, options: {} },
      { id: 'spl-1', kind: 'spl', x: 8, y: 0, w: 4, h: 3, options: { weighting: 'a' } },
      {
        id: 'leq-1',
        kind: 'leq',
        x: 8,
        y: 3,
        w: 4,
        h: 2,
        options: { leqId: 'leq-default' },
      },
      { id: 'spec-1', kind: 'spectrograph', x: 0, y: 5, w: 10, h: 4, options: {} },
      { id: 'bar-1', kind: 'bargraph', x: 10, y: 5, w: 2, h: 4, options: {} },
    ],
  };
}

export function defaultLeqs(): LeqSpec[] {
  return [
    {
      id: 'leq-default',
      label: '',
      weighting: 'a',
      window: { kind: 'sliding', seconds: 300 },
    },
  ];
}

interface Store {
  ready: boolean;
  version: string;
  error: string | null;

  hosts: HostInfo[];
  devices: DeviceInfo[];
  status: SessionStatus;
  selectedHost: string | null;
  selectedDevice: string | null;
  selectedRate: number | null;

  config: EngineConfig;
  plan: BandPlan | null;
  outputs: DeviceInfo[];
  generator: GeneratorConfig;
  generatorChannel: number;
  transfer: TransferConfig;
  transferPlan: TransferPlan | null;
  calibration: Calibration | null;
  calibrationTargets: CalibrationTarget[];

  layout: Layout;
  /** Tile whose settings panel is open, if any. */
  editing: string | null;

  init: () => Promise<void>;
  setError: (e: string | null) => void;
  refreshDevices: (host?: string | null) => Promise<void>;
  selectHost: (host: string | null) => Promise<void>;
  selectDevice: (device: string | null) => void;
  selectRate: (rate: number | null) => void;
  start: () => Promise<void>;
  stop: () => Promise<void>;
  setConfig: (update: (c: EngineConfig) => EngineConfig) => Promise<void>;
  refreshCalibration: () => Promise<void>;
  resetMeasurement: () => Promise<void>;
  resetPeakHold: () => Promise<void>;
  setGenerator: (update: (g: GeneratorConfig) => GeneratorConfig) => Promise<void>;
  setGeneratorChannel: (channel: number) => Promise<void>;
  setReference: (reference: ReferenceSource) => Promise<void>;
  setTransfer: (update: (t: TransferConfig) => TransferConfig) => Promise<void>;
  resetTransfer: () => Promise<void>;

  addLeq: (spec: Omit<LeqSpec, 'id'>) => Promise<string>;
  updateLeq: (id: string, patch: Partial<LeqSpec>) => Promise<void>;
  removeLeq: (id: string) => Promise<void>;

  addTile: (kind: TileKind) => void;
  removeTile: (id: string) => void;
  moveTile: (id: string, x: number, y: number) => void;
  resizeTile: (id: string, w: number, h: number) => void;
  setTileOptions: (id: string, options: Record<string, unknown>) => void;
  setEditing: (id: string | null) => void;
  resetLayout: () => void;
}

/**
 * Layout writes are debounced.
 *
 * Dragging a tile produces a position update on every pointer move; writing the
 * settings file each time would put a few hundred file writes into a single
 * gesture. The delay is short enough that closing the window straight after a
 * drag still saves.
 */
let saveTimer: ReturnType<typeof setTimeout> | null = null;
function scheduleSave(layout: Layout) {
  if (saveTimer) clearTimeout(saveTimer);
  saveTimer = setTimeout(() => {
    saveTimer = null;
    void api.saveLayout(layout).catch(() => {
      // A failed layout save is not worth interrupting a measurement over. The
      // next change tries again.
    });
  }, 400);
}

function isLayout(v: unknown): v is Layout {
  if (!v || typeof v !== 'object') return false;
  const l = v as Layout;
  return Array.isArray(l.tiles) && typeof l.cols === 'number';
}

export const useStore = create<Store>((set, get) => ({
  ready: false,
  version: '',
  error: null,

  hosts: [],
  devices: [],
  status: {
    running: false,
    droppedFrames: 0,
    streamErrors: 0,
    referenceUnderruns: 0,
    reference: { kind: 'off' },
    clockShared: false,
  },
  selectedHost: null,
  selectedDevice: null,
  selectedRate: null,

  config: DEFAULT_ENGINE_CONFIG,
  plan: null,
  outputs: [],
  generator: DEFAULT_GENERATOR,
  generatorChannel: 0,
  transfer: DEFAULT_TRANSFER,
  transferPlan: null,
  calibration: null,
  calibrationTargets: [],

  layout: defaultLayout(),
  editing: null,

  async init() {
    try {
      const s = await api.startup();
      const config: EngineConfig = {
        ...s.settings.engine,
        // A settings file from before any LEQ was defined would leave the
        // default LEQ tile pointing at nothing.
        leqs: s.settings.engine.leqs?.length ? s.settings.engine.leqs : defaultLeqs(),
      };
      const layout = isLayout(s.settings.layout) ? s.settings.layout : defaultLayout();

      set({
        ready: true,
        version: s.version,
        hosts: s.hosts,
        devices: s.devices,
        status: s.status,
        plan: s.plan,
        transferPlan: s.transferPlan,
        outputs: s.outputs,
        generator: s.settings.generator ?? DEFAULT_GENERATOR,
        generatorChannel: s.settings.generatorChannel ?? 0,
        transfer: s.settings.transfer ?? DEFAULT_TRANSFER,
        calibrationTargets: s.calibrationTargets,
        selectedHost: s.settings.host,
        selectedDevice: s.settings.device,
        selectedRate: s.settings.sampleRate,
        config,
        layout,
      });

      if (config.leqs !== s.settings.engine.leqs) {
        await get().setConfig(() => config);
      }
      await get().refreshCalibration();
    } catch (e) {
      set({ ready: true, error: errorText(e) });
    }
  },

  setError: (error) => set({ error }),

  async refreshDevices(host) {
    try {
      const devices = await api.listDevices(host ?? get().selectedHost ?? undefined);
      set({ devices });
    } catch (e) {
      set({ devices: [], error: errorText(e) });
    }
  },

  async selectHost(host) {
    set({ selectedHost: host, selectedDevice: null });
    await get().refreshDevices(host);
  },

  selectDevice: (selectedDevice) => set({ selectedDevice }),
  selectRate: (selectedRate) => set({ selectedRate }),

  async start() {
    const { selectedHost, selectedDevice, selectedRate } = get();
    try {
      const status = await api.start({
        host: selectedHost,
        device: selectedDevice,
        sampleRate: selectedRate,
      });
      set({ status, error: null });
      // The device may have opened at a different rate than requested, which
      // rebuilds the band plan.
      const plan = await api.bandPlan();
      set({ plan });
      await get().refreshCalibration();
    } catch (e) {
      set({ error: errorText(e) });
    }
  },

  async stop() {
    try {
      set({ status: await api.stop() });
    } catch (e) {
      set({ error: errorText(e) });
    }
  },

  async setConfig(update) {
    const config = update(get().config);
    set({ config });
    try {
      const plan = await api.setConfig(config);
      set({ plan, error: null });
    } catch (e) {
      set({ error: errorText(e) });
    }
  },

  async refreshCalibration() {
    try {
      set({ calibration: await api.currentCalibration() });
    } catch {
      set({ calibration: null });
    }
  },

  async resetMeasurement() {
    try {
      await api.resetMeasurement();
      set({ error: null });
    } catch (e) {
      set({ error: errorText(e) });
    }
  },

  async resetPeakHold() {
    try {
      await api.resetPeakHold();
    } catch (e) {
      set({ error: errorText(e) });
    }
  },

  async setGenerator(update) {
    const generator = update(get().generator);
    set({ generator });
    try {
      await api.setGenerator(generator, get().generatorChannel);
    } catch (e) {
      set({ error: errorText(e) });
    }
  },

  async setGeneratorChannel(generatorChannel) {
    set({ generatorChannel });
    try {
      await api.setGenerator(get().generator, generatorChannel);
    } catch (e) {
      set({ error: errorText(e) });
    }
  },

  async setReference(reference) {
    try {
      set({ status: await api.setReference(reference) });
    } catch (e) {
      set({ error: errorText(e) });
    }
  },

  async setTransfer(update) {
    const transfer = update(get().transfer);
    set({ transfer });
    try {
      set({ transferPlan: await api.setTransferConfig(transfer) });
    } catch (e) {
      set({ error: errorText(e) });
    }
  },

  async resetTransfer() {
    try {
      await api.resetTransfer();
    } catch (e) {
      set({ error: errorText(e) });
    }
  },

  async addLeq(spec) {
    const id = `leq-${Date.now().toString(36)}-${nextId++}`;
    await get().setConfig((c) => ({ ...c, leqs: [...c.leqs, { ...spec, id }] }));
    return id;
  },

  async updateLeq(id, patch) {
    await get().setConfig((c) => ({
      ...c,
      leqs: c.leqs.map((l) => (l.id === id ? { ...l, ...patch } : l)),
    }));
  },

  async removeLeq(id) {
    await get().setConfig((c) => ({ ...c, leqs: c.leqs.filter((l) => l.id !== id) }));
  },

  addTile(kind) {
    const { layout } = get();
    const size = DEFAULT_SIZE[kind];
    // Drop it below everything else rather than trying to find a gap: a tile
    // that appears somewhere unexpected is worse than one that appears at the
    // bottom, and the user is about to drag it anyway.
    const bottom = layout.tiles.reduce((m, t) => Math.max(m, t.y + t.h), 0);
    const tile: Tile = {
      id: newTileId(kind),
      kind,
      x: 0,
      y: bottom,
      w: Math.min(size.w, layout.cols),
      h: size.h,
      options: {},
    };
    const next = { ...layout, tiles: [...layout.tiles, tile] };
    set({ layout: next, editing: tile.id });
    scheduleSave(next);
  },

  removeTile(id) {
    const layout = get().layout;
    const next = { ...layout, tiles: layout.tiles.filter((t) => t.id !== id) };
    set({ layout: next, editing: get().editing === id ? null : get().editing });
    scheduleSave(next);
  },

  moveTile(id, x, y) {
    const layout = get().layout;
    const next = {
      ...layout,
      tiles: layout.tiles.map((t) =>
        t.id === id
          ? { ...t, x: Math.max(0, Math.min(layout.cols - t.w, x)), y: Math.max(0, y) }
          : t,
      ),
    };
    set({ layout: next });
    scheduleSave(next);
  },

  resizeTile(id, w, h) {
    const layout = get().layout;
    const next = {
      ...layout,
      tiles: layout.tiles.map((t) => {
        if (t.id !== id) return t;
        const min = MIN_SIZE[t.kind];
        return {
          ...t,
          w: Math.max(min.w, Math.min(layout.cols - t.x, w)),
          h: Math.max(min.h, h),
        };
      }),
    };
    set({ layout: next });
    scheduleSave(next);
  },

  setTileOptions(id, options) {
    const layout = get().layout;
    const next = {
      ...layout,
      tiles: layout.tiles.map((t) => (t.id === id ? { ...t, options: { ...t.options, ...options } } : t)),
    };
    set({ layout: next });
    scheduleSave(next);
  },

  setEditing: (editing) => set({ editing }),

  resetLayout() {
    const next = defaultLayout();
    set({ layout: next, editing: null });
    scheduleSave(next);
  },
}));
