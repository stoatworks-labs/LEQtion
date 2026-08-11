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
  type ProjectSummary,
  type ReferenceSource,
  type SessionStatus,
  type ShowSummary,
  type TransferConfig,
  type TransferPlan,
  DEFAULT_GENERATOR,
  DEFAULT_TRANSFER,
} from '../types';

export type TileKind =
  | 'rta'
  | 'chart'
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
  chart: { w: 4, h: 2 },
  transfer: { w: 4, h: 3 },
  generator: { w: 3, h: 2 },
};

export const DEFAULT_SIZE: Record<TileKind, { w: number; h: number }> = {
  rta: { w: 8, h: 5 },
  spectrograph: { w: 8, h: 4 },
  bargraph: { w: 2, h: 5 },
  spl: { w: 4, h: 3 },
  leq: { w: 2, h: 2 },
  chart: { w: 8, h: 4 },
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

  projects: ProjectSummary[];
  projectsRoot: string;
  /** The open project, or null. Measuring never requires one. */
  project: ProjectSummary | null;
  shows: ShowSummary[];
  activeShow: ShowSummary | null;
  /**
   * Whether anything has changed since the active show was loaded or saved.
   *
   * Deliberately coarse — it is set by any configuration or layout change and
   * cleared by a save or a load. It answers "is there something to save?", which is
   * the only question the UI asks, and it errs towards saying yes.
   */
  showChanged: boolean;

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

  refreshProjects: () => Promise<void>;
  createProject: (name: string) => Promise<void>;
  openProject: (dir: string) => Promise<void>;
  closeProject: () => Promise<void>;
  renameProject: (name: string) => Promise<void>;
  deleteProject: (dir: string) => Promise<string | null>;

  saveShowAs: (name: string) => Promise<void>;
  updateActiveShow: () => Promise<void>;
  loadShow: (id: string) => Promise<void>;
  renameShow: (id: string, name: string) => Promise<void>;
  deleteShow: (id: string) => Promise<string | null>;
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

  projects: [],
  projectsRoot: '',
  project: null,
  shows: [],
  activeShow: null,
  showChanged: false,

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
        projects: s.projects,
        projectsRoot: s.projectsRoot,
        project: s.project,
        shows: s.shows,
        activeShow: s.shows.find((sh) => sh.id === s.settings.lastShow) ?? null,
        // Nothing has been touched yet, whatever was restored.
        showChanged: false,
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
    set({ selectedHost: host, selectedDevice: null, showChanged: true });
    await get().refreshDevices(host);
  },

  selectDevice: (selectedDevice) => set({ selectedDevice, showChanged: true }),
  selectRate: (selectedRate) => set({ selectedRate, showChanged: true }),

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
    set({ config, showChanged: true });
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
    set({ generator, showChanged: true });
    try {
      await api.setGenerator(generator, get().generatorChannel);
    } catch (e) {
      set({ error: errorText(e) });
    }
  },

  async setGeneratorChannel(generatorChannel) {
    set({ generatorChannel, showChanged: true });
    try {
      await api.setGenerator(get().generator, generatorChannel);
    } catch (e) {
      set({ error: errorText(e) });
    }
  },

  async setReference(reference) {
    try {
      set({ status: await api.setReference(reference), showChanged: true });
    } catch (e) {
      set({ error: errorText(e) });
    }
  },

  async setTransfer(update) {
    const transfer = update(get().transfer);
    set({ transfer, showChanged: true });
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
    set({ layout: next, editing: tile.id, showChanged: true });
    scheduleSave(next);
  },

  removeTile(id) {
    const layout = get().layout;
    const next = { ...layout, tiles: layout.tiles.filter((t) => t.id !== id) };
    set({ layout: next, editing: get().editing === id ? null : get().editing, showChanged: true });
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
    set({ layout: next, showChanged: true });
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
    set({ layout: next, showChanged: true });
    scheduleSave(next);
  },

  setTileOptions(id, options) {
    const layout = get().layout;
    const next = {
      ...layout,
      tiles: layout.tiles.map((t) => (t.id === id ? { ...t, options: { ...t.options, ...options } } : t)),
    };
    set({ layout: next, showChanged: true });
    scheduleSave(next);
  },

  setEditing: (editing) => set({ editing }),

  resetLayout() {
    const next = defaultLayout();
    set({ layout: next, editing: null, showChanged: true });
    scheduleSave(next);
  },

  // -- projects and shows ---------------------------------------------------
  //
  // A show is applied by Rust, not here: `loadShow` sends an id and gets the whole
  // new state back. The frontend never holds a show's configuration, so it cannot
  // apply half of one and leave the engine and the tiles disagreeing about what is
  // being measured.

  async refreshProjects() {
    try {
      set({ projects: await api.listProjects() });
    } catch (e) {
      set({ error: errorText(e) });
    }
  },

  async createProject(name) {
    try {
      const project = await api.createProject(name);
      set({ project, shows: [], activeShow: null, error: null });
      await get().refreshProjects();
    } catch (e) {
      set({ error: errorText(e) });
    }
  },

  async openProject(dir) {
    try {
      const { project, shows } = await api.openProject(dir);
      // Opening a project does not load a show, so whatever is being measured
      // carries on untouched and there is no active show until one is chosen.
      set({ project, shows, activeShow: null, error: null });
    } catch (e) {
      set({ error: errorText(e) });
    }
  },

  async closeProject() {
    try {
      await api.closeProject();
      set({ project: null, shows: [], activeShow: null, error: null });
    } catch (e) {
      set({ error: errorText(e) });
    }
  },

  async renameProject(name) {
    const project = get().project;
    if (!project) return;
    try {
      const renamed = await api.renameProject(project.dir, name);
      set({ project: renamed, error: null });
      await get().refreshProjects();
    } catch (e) {
      set({ error: errorText(e) });
    }
  },

  /** Returns where the project was moved to, for the UI to report. */
  async deleteProject(dir) {
    try {
      const movedTo = await api.deleteProject(dir);
      if (get().project?.dir === dir) {
        set({ project: null, shows: [], activeShow: null });
      }
      set({ error: null });
      await get().refreshProjects();
      return movedTo;
    } catch (e) {
      set({ error: errorText(e) });
      return null;
    }
  },

  async saveShowAs(name) {
    const project = get().project;
    if (!project) {
      set({ error: 'Open or create a project before saving a show.' });
      return;
    }
    try {
      const show = await api.saveShow(project.dir, name);
      set({
        shows: await api.listShows(project.dir),
        activeShow: show,
        showChanged: false,
        error: null,
      });
      await get().refreshProjects();
    } catch (e) {
      set({ error: errorText(e) });
    }
  },

  async updateActiveShow() {
    const { project, activeShow } = get();
    if (!project || !activeShow) return;
    try {
      const show = await api.updateShow(project.dir, activeShow.id);
      set({
        shows: await api.listShows(project.dir),
        activeShow: show,
        showChanged: false,
        error: null,
      });
    } catch (e) {
      set({ error: errorText(e) });
    }
  },

  async loadShow(id) {
    const project = get().project;
    if (!project) return;
    try {
      const applied = await api.loadShow(project.dir, id);
      const layout = isLayout(applied.settings.layout)
        ? applied.settings.layout
        : defaultLayout();

      set({
        activeShow: applied.show,
        config: {
          ...applied.settings.engine,
          leqs: applied.settings.engine.leqs?.length
            ? applied.settings.engine.leqs
            : defaultLeqs(),
        },
        transfer: applied.settings.transfer ?? DEFAULT_TRANSFER,
        // The generator comes back silent whatever the show was saved with — the
        // backend forces it, and the level is what is actually restored. Loading a
        // show is not a reason to put a signal into a PA.
        generator: applied.settings.generator ?? DEFAULT_GENERATOR,
        generatorChannel: applied.settings.generatorChannel ?? 0,
        selectedHost: applied.settings.host,
        selectedDevice: applied.settings.device,
        selectedRate: applied.settings.sampleRate,
        plan: applied.plan,
        transferPlan: applied.transferPlan,
        status: applied.status,
        // Not scheduled for saving: Rust has already written this layout as part
        // of applying the show, and echoing it back would be a redundant write.
        layout,
        editing: null,
        showChanged: false,
        error: null,
      });
      // The device may differ from the one that is open, so the calibration shown
      // has to be re-read rather than assumed — it belongs to the hardware, not to
      // the show. See docs/tuning.md §1.1.
      await get().refreshCalibration();
    } catch (e) {
      set({ error: errorText(e) });
    }
  },

  async renameShow(id, name) {
    const project = get().project;
    if (!project) return;
    try {
      const show = await api.renameShow(project.dir, id, name);
      set({
        shows: await api.listShows(project.dir),
        activeShow: get().activeShow?.id === id ? show : get().activeShow,
        error: null,
      });
    } catch (e) {
      set({ error: errorText(e) });
    }
  },

  async deleteShow(id) {
    const project = get().project;
    if (!project) return null;
    try {
      const movedTo = await api.deleteShow(project.dir, id);
      set({
        shows: await api.listShows(project.dir),
        activeShow: get().activeShow?.id === id ? null : get().activeShow,
        error: null,
      });
      await get().refreshProjects();
      return movedTo;
    } catch (e) {
      set({ error: errorText(e) });
      return null;
    }
  },
}));
