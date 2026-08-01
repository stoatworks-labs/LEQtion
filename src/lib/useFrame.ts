import { useEffect, useRef, useState } from 'react';

import { currentFrame, onFrame } from './ipc';
import type { Frame } from '../types';

/**
 * The latest frame, in a ref, without causing a re-render.
 *
 * For canvas tiles. They redraw on their own animation frame and want whatever
 * is current at that instant; re-rendering them thirty times a second would
 * achieve nothing except work.
 */
export function useFrameRef(): React.RefObject<Frame | null> {
  const ref = useRef<Frame | null>(currentFrame());
  useEffect(() => onFrame((f) => (ref.current = f)), []);
  return ref;
}

/**
 * The latest frame as state, at a rate a person can actually read.
 *
 * For numeric readouts. A level that updates thirty times a second is a blur;
 * ten is fast enough to look live and slow enough to read the last digit. The
 * *measurement* is unaffected either way — integration happens in Rust on every
 * sample, not on whatever the display last saw.
 */
export function useFrameValue(intervalMs = 100): Frame | null {
  const [frame, setFrame] = useState<Frame | null>(currentFrame());
  const last = useRef(0);

  useEffect(
    () =>
      onFrame((f) => {
        const now = performance.now();
        if (now - last.current >= intervalMs) {
          last.current = now;
          setFrame(f);
        }
      }),
    [intervalMs],
  );

  return frame;
}

/**
 * Run a draw callback on every animation frame while mounted.
 *
 * `requestAnimationFrame` rather than drawing on frame arrival: the display
 * refreshes when the compositor says so, and painting more often than that is
 * wasted. It also means a tile scrolled out of view or in a hidden window stops
 * drawing, because the browser stops calling back.
 */
export function useAnimationFrame(draw: () => void): void {
  const saved = useRef(draw);
  saved.current = draw;

  useEffect(() => {
    let handle = 0;
    const tick = () => {
      saved.current();
      handle = requestAnimationFrame(tick);
    };
    handle = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(handle);
  }, []);
}
