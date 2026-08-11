import { useCallback, useLayoutEffect, useRef, useState } from 'react';

import { GRID_COLS, GRID_GAP, ROW_HEIGHT, useStore, type Tile } from '../state/store';
import { tileType } from '../tiles/registry';

import { ErrorBoundary } from './ErrorBoundary';

/**
 * The configurable tile grid.
 *
 * Hand-rolled rather than a grid library. The requirement is a fixed-column
 * grid with drag-to-move and drag-to-resize, and the components inside are
 * canvases that must not be re-mounted or transformed while they draw — which
 * rules out most of the libraries, since their usual trick is to re-parent or
 * CSS-transform a tile mid-drag. Doing it here is about two hundred lines and
 * keeps the canvases still.
 *
 * ## Overlap
 *
 * Tiles are allowed to overlap. Auto-packing them apart — the thing every grid
 * library does by default — means dropping a tile shoves three others somewhere
 * the user did not ask for, and on a measurement dashboard that someone has
 * arranged deliberately, that is worse than letting them make a mess they can
 * see and fix. Position is exactly what was dragged.
 */

type Drag =
  | { mode: 'move'; id: string; startX: number; startY: number; originX: number; originY: number }
  | { mode: 'resize'; id: string; startX: number; startY: number; originW: number; originH: number };

export function TileGrid() {
  const layout = useStore((s) => s.layout);
  const moveTile = useStore((s) => s.moveTile);
  const resizeTile = useStore((s) => s.resizeTile);
  const container = useRef<HTMLDivElement>(null);
  const [colWidth, setColWidth] = useState(100);
  const drag = useRef<Drag | null>(null);

  // Column width has to be measured, because the grid is fluid: the cell size a
  // drag converts pixels into is whatever the window is now, not whatever it
  // was when the layout was saved.
  useLayoutEffect(() => {
    const el = container.current;
    if (!el) return;
    const measure = () => {
      const w = el.clientWidth;
      setColWidth((w - GRID_GAP * (GRID_COLS - 1)) / GRID_COLS);
    };
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const onPointerMove = useCallback(
    (e: PointerEvent) => {
      const d = drag.current;
      if (!d) return;
      const dx = e.clientX - d.startX;
      const dy = e.clientY - d.startY;
      const cellX = Math.round(dx / (colWidth + GRID_GAP));
      const cellY = Math.round(dy / (ROW_HEIGHT + GRID_GAP));

      if (d.mode === 'move') {
        moveTile(d.id, d.originX + cellX, d.originY + cellY);
      } else {
        resizeTile(d.id, d.originW + cellX, d.originH + cellY);
      }
    },
    [colWidth, moveTile, resizeTile],
  );

  const endDrag = useCallback(() => {
    drag.current = null;
    document.body.classList.remove('dragging');
    window.removeEventListener('pointermove', onPointerMove);
    window.removeEventListener('pointerup', endDrag);
  }, [onPointerMove]);

  const beginDrag = useCallback(
    (d: Drag) => {
      drag.current = d;
      document.body.classList.add('dragging');
      window.addEventListener('pointermove', onPointerMove);
      window.addEventListener('pointerup', endDrag);
    },
    [endDrag, onPointerMove],
  );

  const rows = Math.max(6, ...layout.tiles.map((t) => t.y + t.h));

  return (
    <div
      ref={container}
      className="grid"
      style={{
        gridTemplateColumns: `repeat(${GRID_COLS}, minmax(0, 1fr))`,
        // Explicit rows rather than a min-height. A min-height on the element
        // that also carries `overflow: auto` makes the scroll container itself
        // grow instead of scrolling, so a tile dragged — or added — below the
        // fold became unreachable: no scrollbar, and the wheel did nothing.
        // Declaring the rows makes the same extent *content*, which scrolls.
        gridTemplateRows: `repeat(${rows}, ${ROW_HEIGHT}px)`,
        gridAutoRows: `${ROW_HEIGHT}px`,
        gap: GRID_GAP,
      }}
    >
      {layout.tiles.map((tile) => (
        <TileFrame key={tile.id} tile={tile} onBeginDrag={beginDrag} />
      ))}
      {layout.tiles.length === 0 && (
        <p className="grid-empty">
          No tiles. Add one from the toolbar, or reset to the default layout.
        </p>
      )}
    </div>
  );
}

function TileFrame({ tile, onBeginDrag }: { tile: Tile; onBeginDrag: (d: Drag) => void }) {
  const editing = useStore((s) => s.editing === tile.id);
  const setEditing = useStore((s) => s.setEditing);
  const removeTile = useStore((s) => s.removeTile);
  const type = tileType(tile.kind);

  return (
    <section
      className="tile"
      style={{
        gridColumn: `${tile.x + 1} / span ${tile.w}`,
        gridRow: `${tile.y + 1} / span ${tile.h}`,
      }}
    >
      <header
        className="tile-head"
        onPointerDown={(e) => {
          // Only the header drags, and only with the primary button — otherwise
          // every click on a select inside a tile would start a move.
          if (e.button !== 0) return;
          if ((e.target as HTMLElement).closest('button')) return;
          e.preventDefault();
          onBeginDrag({
            mode: 'move',
            id: tile.id,
            startX: e.clientX,
            startY: e.clientY,
            originX: tile.x,
            originY: tile.y,
          });
        }}
      >
        <h2>{type.title}</h2>
        <div className="tile-actions">
          <button
            type="button"
            className="icon"
            aria-label={editing ? 'Close settings' : 'Tile settings'}
            aria-pressed={editing}
            onClick={() => setEditing(editing ? null : tile.id)}
          >
            ⚙
          </button>
          <button
            type="button"
            className="icon"
            aria-label="Remove tile"
            onClick={() => removeTile(tile.id)}
          >
            ✕
          </button>
        </div>
      </header>

      {/* Per tile, so one tile that cannot draw does not stop its neighbours
          from updating. Keyed on the mode: switching between settings and body
          clears a previous failure rather than leaving the frame stuck. */}
      <ErrorBoundary key={editing ? 'settings' : 'body'} label={type.title}>
        {editing ? (
          <div className="tile-settings">
            <type.Settings tile={tile} />
          </div>
        ) : (
          <type.Body tile={tile} />
        )}
      </ErrorBoundary>

      <button
        type="button"
        className="tile-resize"
        aria-label="Resize tile"
        onPointerDown={(e) => {
          if (e.button !== 0) return;
          e.preventDefault();
          onBeginDrag({
            mode: 'resize',
            id: tile.id,
            startX: e.clientX,
            startY: e.clientY,
            originW: tile.w,
            originH: tile.h,
          });
        }}
      />
    </section>
  );
}
