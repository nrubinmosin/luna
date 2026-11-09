import { useEffect, useLayoutEffect, useRef, type RefObject } from 'react';

/**
 * Closes a popover placed once beside its anchor (portaled, position: fixed)
 * once that placing no longer holds: something holding the anchor scrolled it
 * away (a dialog's body, a Settings page, the sidebar), or the window was
 * resized, which can leave the popover past the new edge though the anchor
 * stayed put. Left open, it would stand over the wrong row or off the window.
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
    const scrolled = () => {
      const now = el.getBoundingClientRect();
      if (Math.abs(now.top - was.top) > 0.5 || Math.abs(now.left - was.left) > 0.5) latest.current();
    };
    const resized = () => latest.current();
    // Capture: a scroll does not bubble, and any scroller may hold the anchor.
    window.addEventListener('scroll', scrolled, true);
    window.addEventListener('resize', resized);
    return () => {
      window.removeEventListener('scroll', scrolled, true);
      window.removeEventListener('resize', resized);
    };
  }, [open, anchor]);
}
