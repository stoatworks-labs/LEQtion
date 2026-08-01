import type { ComponentType } from 'react';

import type { Tile, TileKind } from '../state/store';

import { BargraphSettings, BargraphTile } from './BargraphTile';
import { LeqSettings, LeqTile } from './LeqTile';
import { RtaSettings, RtaTile } from './RtaTile';
import { SpectrographSettings, SpectrographTile } from './SpectrographTile';
import { SplSettings, SplTile } from './SplTile';

/**
 * The one place a tile kind is defined.
 *
 * Adding a kind means adding an entry here and nothing else — the grid, the
 * "add tile" menu and the settings panel are all driven from this table, so
 * there is no second list to forget to update.
 */
export interface TileType {
  kind: TileKind;
  title: string;
  blurb: string;
  Body: ComponentType<{ tile: Tile }>;
  Settings: ComponentType<{ tile: Tile }>;
}

export const TILE_TYPES: TileType[] = [
  {
    kind: 'rta',
    title: 'RTA',
    blurb: 'Fractional-octave spectrum, 1/1 to 1/48.',
    Body: RtaTile,
    Settings: RtaSettings,
  },
  {
    kind: 'spectrograph',
    title: 'Spectrograph',
    blurb: 'The same bands over time, scrolling.',
    Body: SpectrographTile,
    Settings: SpectrographSettings,
  },
  {
    kind: 'bargraph',
    title: 'Bargraph',
    blurb: 'Level meter with held maximum and an input peak strip.',
    Body: BargraphTile,
    Settings: BargraphSettings,
  },
  {
    kind: 'spl',
    title: 'SPL',
    blurb: 'Time-weighted level, with max, min and peak.',
    Body: SplTile,
    Settings: SplSettings,
  },
  {
    kind: 'leq',
    title: 'LEQ',
    blurb: 'One equivalent level, on the window and weighting you define.',
    Body: LeqTile,
    Settings: LeqSettings,
  },
];

export function tileType(kind: TileKind): TileType {
  const t = TILE_TYPES.find((x) => x.kind === kind);
  if (!t) throw new Error(`unknown tile kind: ${kind}`);
  return t;
}
