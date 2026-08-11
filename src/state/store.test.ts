/**
 * Tests for the project/show half of the store.
 *
 * The rest of the store is a thin pass-through to Rust and is covered by the Rust
 * tests, but these actions make decisions of their own — what to do when there is no
 * project, when to consider the configuration changed, what to clear when a show is
 * deleted — and those decisions are the ones a user notices being wrong.
 *
 * The IPC layer is mocked, so nothing here needs a Tauri window.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';

import {
  DEFAULT_ENGINE_CONFIG,
  DEFAULT_GENERATOR,
  DEFAULT_TRANSFER,
  type BandPlan,
  type SessionStatus,
  type Settings,
  type ShowApplied,
  type ShowSummary,
  type TransferPlan,
} from '../types';

vi.mock('../lib/ipc', () => ({
  api: {
    listProjects: vi.fn(),
    createProject: vi.fn(),
    openProject: vi.fn(),
    closeProject: vi.fn(),
    renameProject: vi.fn(),
    deleteProject: vi.fn(),
    listShows: vi.fn(),
    saveShow: vi.fn(),
    updateShow: vi.fn(),
    loadShow: vi.fn(),
    renameShow: vi.fn(),
    deleteShow: vi.fn(),
    setConfig: vi.fn(),
    currentCalibration: vi.fn(),
    saveLayout: vi.fn(),
  },
  errorText: (e: unknown) => String(e),
}));

import { api } from '../lib/ipc';
import { defaultLayout, useStore } from './store';

const mocked = api as unknown as Record<string, ReturnType<typeof vi.fn>>;

function aShow(over: Partial<ShowSummary> = {}): ShowSummary {
  return {
    id: 'foh-20260811-120000',
    name: 'FOH system',
    created: '2026-08-11T12:00:00Z',
    modified: '2026-08-11T12:00:00Z',
    notes: '',
    device: 'Scarlett 2i2',
    ...over,
  };
}

const A_PLAN: BandPlan = {
  fraction: '1/3',
  fftSize: 16384,
  sampleRate: 96000,
  enbw: 1.5,
  binHz: 5.86,
  bands: [],
  resolvedAboveHz: 100,
};

const A_TRANSFER_PLAN: TransferPlan = {
  pointsPerOctave: 48,
  sampleRate: 96000,
  frequencies: [100, 1000],
  fftSizes: [16384],
  longestWindowSeconds: 1,
};

const A_STATUS: SessionStatus = {
  running: false,
  droppedFrames: 0,
  streamErrors: 0,
  referenceUnderruns: 0,
  reference: { kind: 'internal' },
  clockShared: true,
};

/** A show whose configuration differs from the defaults in every field that matters. */
function anAppliedShow(): ShowApplied {
  const settings: Settings = {
    engine: { ...DEFAULT_ENGINE_CONFIG, leqs: [] },
    host: 'CoreAudio',
    device: 'Scarlett 2i2',
    sampleRate: 96000,
    calibrations: [],
    layout: { cols: 12, tiles: [] },
    transfer: { ...DEFAULT_TRANSFER, pointsPerOctave: 48 },
    // Saved while generating; the backend forces it Off before it comes back.
    generator: { ...DEFAULT_GENERATOR, levelDbfs: -12 },
    generatorChannel: 2,
    reference: { kind: 'internal' },
    lastProject: 'Tour',
    lastShow: 'foh-20260811-120000',
  };
  return {
    show: aShow(),
    settings,
    plan: A_PLAN,
    transferPlan: A_TRANSFER_PLAN,
    status: A_STATUS,
  };
}

const PROJECT = {
  id: 'tour-20260811-090000',
  name: 'Tour',
  dir: 'Tour',
  created: '2026-08-11T09:00:00Z',
  modified: '2026-08-11T09:00:00Z',
  notes: '',
  showCount: 1,
};

beforeEach(() => {
  vi.clearAllMocks();
  mocked.listProjects.mockResolvedValue([PROJECT]);
  mocked.listShows.mockResolvedValue([aShow()]);
  mocked.currentCalibration.mockResolvedValue(null);
  useStore.setState({
    projects: [],
    project: null,
    shows: [],
    activeShow: null,
    showChanged: false,
    error: null,
    config: DEFAULT_ENGINE_CONFIG,
    transfer: DEFAULT_TRANSFER,
    generator: DEFAULT_GENERATOR,
    generatorChannel: 0,
    layout: defaultLayout(),
    selectedHost: null,
    selectedDevice: null,
    selectedRate: null,
  });
});

describe('projects', () => {
  it('opens a project without touching what is being measured', async () => {
    mocked.openProject.mockResolvedValue({ project: PROJECT, shows: [aShow()] });
    const before = useStore.getState().config;

    await useStore.getState().openProject('Tour');

    const s = useStore.getState();
    expect(s.project?.dir).toBe('Tour');
    expect(s.shows).toHaveLength(1);
    // Opening a project is not loading a show. Nothing about the measurement moves.
    expect(s.activeShow).toBeNull();
    expect(s.config).toBe(before);
  });

  it('refuses to save a show with no project open, and says why', async () => {
    await useStore.getState().saveShowAs('FOH');

    expect(mocked.saveShow).not.toHaveBeenCalled();
    expect(useStore.getState().error).toMatch(/project/i);
  });

  it('reports where a deleted project went, because deleting is a move', async () => {
    mocked.deleteProject.mockResolvedValue('/Users/x/Documents/LEQtion/.deleted/Tour-20260811');
    useStore.setState({ project: PROJECT, shows: [aShow()] });

    const movedTo = await useStore.getState().deleteProject('Tour');

    expect(movedTo).toContain('.deleted');
    const s = useStore.getState();
    expect(s.project).toBeNull();
    expect(s.shows).toEqual([]);
    expect(s.activeShow).toBeNull();
  });
});

describe('loading a show', () => {
  it('applies the whole configuration the backend returned', async () => {
    mocked.loadShow.mockResolvedValue(anAppliedShow());
    useStore.setState({ project: PROJECT });

    await useStore.getState().loadShow('foh-20260811-120000');

    const s = useStore.getState();
    expect(s.activeShow?.name).toBe('FOH system');
    expect(s.transfer.pointsPerOctave).toBe(48);
    expect(s.generatorChannel).toBe(2);
    expect(s.selectedHost).toBe('CoreAudio');
    expect(s.selectedDevice).toBe('Scarlett 2i2');
    expect(s.selectedRate).toBe(96000);
    expect(s.plan).toBe(A_PLAN);
    expect(s.transferPlan).toBe(A_TRANSFER_PLAN);
    expect(s.layout).toEqual({ cols: 12, tiles: [] });
  });

  it('is not counted as a change, so there is nothing to save straight after loading', async () => {
    mocked.loadShow.mockResolvedValue(anAppliedShow());
    useStore.setState({ project: PROJECT, showChanged: true });

    await useStore.getState().loadShow('foh-20260811-120000');

    expect(useStore.getState().showChanged).toBe(false);
  });

  /**
   * The calibration belongs to the microphone, not to the show — see
   * `docs/tuning.md` §1.1. A loaded show may name a device that is not plugged in,
   * so what is displayed has to be re-read rather than taken from the show.
   */
  it('re-reads the calibration rather than restoring the show’s', async () => {
    mocked.loadShow.mockResolvedValue(anAppliedShow());
    useStore.setState({ project: PROJECT });

    await useStore.getState().loadShow('foh-20260811-120000');

    expect(mocked.currentCalibration).toHaveBeenCalled();
  });

  it('falls back to the default layout if a show carries a broken one', async () => {
    const applied = anAppliedShow();
    applied.settings.layout = { nonsense: true };
    mocked.loadShow.mockResolvedValue(applied);
    useStore.setState({ project: PROJECT });

    await useStore.getState().loadShow('foh-20260811-120000');

    expect(useStore.getState().layout.tiles.length).toBeGreaterThan(0);
  });

  it('does nothing when no project is open', async () => {
    await useStore.getState().loadShow('foh-20260811-120000');
    expect(mocked.loadShow).not.toHaveBeenCalled();
  });
});

describe('the changed flag', () => {
  it('is raised by a configuration change and cleared by saving', async () => {
    mocked.setConfig.mockResolvedValue(A_PLAN);
    mocked.updateShow.mockResolvedValue(aShow({ modified: '2026-08-11T13:00:00Z' }));
    useStore.setState({ project: PROJECT, activeShow: aShow() });

    expect(useStore.getState().showChanged).toBe(false);
    await useStore.getState().setConfig((c) => ({ ...c, leqs: [] }));
    expect(useStore.getState().showChanged).toBe(true);

    await useStore.getState().updateActiveShow();
    expect(useStore.getState().showChanged).toBe(false);
  });

  it('is raised by moving a tile, because the layout is part of a show', () => {
    const id = useStore.getState().layout.tiles[0].id;
    useStore.getState().moveTile(id, 2, 2);
    expect(useStore.getState().showChanged).toBe(true);
  });

  it('is raised by choosing a different input', () => {
    useStore.getState().selectDevice('Some other interface');
    expect(useStore.getState().showChanged).toBe(true);
  });

  it('is cleared by saving under a new name', async () => {
    mocked.saveShow.mockResolvedValue(aShow({ name: 'Monitors' }));
    useStore.setState({ project: PROJECT, showChanged: true });

    await useStore.getState().saveShowAs('Monitors');

    const s = useStore.getState();
    expect(s.showChanged).toBe(false);
    expect(s.activeShow?.name).toBe('Monitors');
  });
});

describe('deleting a show', () => {
  it('clears the active show only when it was the one deleted', async () => {
    mocked.deleteShow.mockResolvedValue('/Users/x/.deleted/foh.json');
    mocked.listShows.mockResolvedValue([]);
    useStore.setState({ project: PROJECT, activeShow: aShow({ id: 'other-show' }) });

    await useStore.getState().deleteShow('foh-20260811-120000');
    expect(useStore.getState().activeShow?.id).toBe('other-show');

    useStore.setState({ activeShow: aShow({ id: 'doomed' }) });
    await useStore.getState().deleteShow('doomed');
    expect(useStore.getState().activeShow).toBeNull();
  });
});
