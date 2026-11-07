import type { Terminal as XTerm } from '@xterm/xterm';
import type { FitAddon } from '@xterm/addon-fit';
import { safely } from './safely';

/**
 * Keeps a terminal fitted to its box and the pty told its size: `refit` after
 * a move, `nudgeRepaint` when the CLI has to paint again, and the box watched
 * for size changes. `canFit` says the terminal is alive and in a pane;
 * `alive` only the first.
 */
export function sizing(
  term: XTerm,
  fit: FitAddon,
  host: HTMLElement,
  {
    alive,
    canFit,
    resize
  }: { alive: () => boolean; canFit: () => boolean; resize: (cols: number, rows: number) => void }
) {
  let repaintTimer: ReturnType<typeof setTimeout> | undefined;

  // Fitting a detached or zero-sized host throws; that happens on every
  // pane close and used to take the whole window down with it. Parked is the
  // other case to sit out: the lot's box is not the pane's, and measuring it
  // would resize the pty to a size nobody is looking at.
  const refit = () => {
    if (!canFit() || !host.isConnected || !host.clientWidth || !host.clientHeight) return;
    safely(() => fit.fit());
  };
  refit();
  // The first fit runs before the stylesheet and webfont have had their say,
  // so the height it measures is not the final one. Measure again once the
  // browser has laid the pane out for real.
  requestAnimationFrame(refit);
  void document.fonts?.ready.then(refit).catch(() => {});

  /**
   * Makes the CLI redraw its screen. It runs fullscreen, i.e. on the
   * alternate screen, and only paints on input or on SIGWINCH — so anything
   * that leaves this terminal holding a stale or empty frame has to ask for
   * a repaint rather than wait for one. Resizing to a different size and
   * straight back is what forces it.
   */
  const nudgeRepaint = () => {
    if (!alive()) return;
    safely(() => term.refresh(0, term.rows - 1));
    resize(term.cols, Math.max(1, term.rows - 1));
    clearTimeout(repaintTimer);
    repaintTimer = setTimeout(() => {
      if (!alive()) return;
      // Re-read the size instead of restoring the one captured above: the
      // layout can settle within these 60ms (the metadata row rewrapping is
      // enough), and sending a stale row count leaves the pty believing the
      // viewport is taller than it is — which pushes the CLI's input box
      // below the bottom of the pane until the pane is reopened.
      refit();
      resize(term.cols, term.rows);
    }, 60);
  };

  // At most one fit per 80ms while a size is animating (the sidebar sliding,
  // a split dragged), plus one when it settles: every fit is a pty resize, and
  // every resize makes the CLI repaint its whole screen.
  let lastFit = 0;
  let fitTimer: ReturnType<typeof setTimeout> | undefined;
  const ro = new ResizeObserver(() => {
    clearTimeout(fitTimer);
    const now = performance.now();
    if (now - lastFit >= 80) {
      lastFit = now;
      refit();
    }
    fitTimer = setTimeout(() => {
      lastFit = performance.now();
      refit();
    }, 90);
  });
  ro.observe(host);

  return {
    refit,
    nudgeRepaint,
    dispose: () => {
      clearTimeout(repaintTimer);
      safely(() => ro.disconnect());
    }
  };
}
