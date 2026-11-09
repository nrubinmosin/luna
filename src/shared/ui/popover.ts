import { useEffect, useLayoutEffect, useRef, type RefObject } from 'react';

/**
 * Closes a popover placed once beside its anchor (portaled, position: fixed)
 * as soon as the anchor moves from under it: something holding the anchor
 * scrolled (a dialog's body, a Settings page, the sidebar) or the window was
 * resized. Left open, it would stay where it was placed, over the wrong row.
 * A scroll inside the popover itself moves no anchor and closes nothing.
 */
export function useCloseWhenMoved(
  open: boolean,
  anchor: RefObject<Element | null> | Element | null,
  close: () => void
) {
  // The latest close, without opening the watch again on every render.
  const latest = useRef(close);
  useLayoutEffect(() => {
    latest.current = close;
  });
  useEffect(() => {
    const el = anchor instanceof Element ? anchor : anchor?.current;
    if (!open || !el) return;
    const was = el.getBoundingClientRect();
    const check = () => {
      const now = el.getBoundingClientRect();
      if (Math.abs(now.top - was.top) > 0.5 || Math.abs(now.left - was.left) > 0.5) latest.current();
    };
    // Capture: a scroll does not bubble, and any scroller may hold the anchor.
    window.addEventListener('scroll', check, true);
    window.addEventListener('resize', check);
    return () => {
      window.removeEventListener('scroll', check, true);
      window.removeEventListener('resize', check);
    };
  }, [open, anchor]);
}
