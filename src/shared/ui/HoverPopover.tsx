import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type CSSProperties,
  type FocusEvent,
  type HTMLAttributes,
  type PointerEvent,
  type ReactNode
} from 'react';
import { createPortal } from 'react-dom';
import { useDialogHost } from './dialogHost';
import { useCloseWhenMoved } from './popover';

/** How long the pointer rests on the face before the panel opens: passing over
 *  it on the way somewhere else opens nothing. */
export const OPEN_DELAY_MS = 250;
/** How long the panel stays once the pointer has left the face or the panel:
 *  long enough to cross the gap and reach its buttons. */
export const CLOSE_GRACE_MS = 400;

/** How it was opened, which decides what closes it: a hover ends with the
 *  pointer leaving, focus with focus leaving, a click (pinned) only with
 *  another click, Escape or a click elsewhere. */
type Mode = 'hover' | 'focus' | 'pinned';

const GAP = 6;
const EDGE = 8;

/** What Tab stops at in the panel, in order. */
const focusables = (el: HTMLElement | null) => [
  ...(el?.querySelectorAll<HTMLElement>(
    'button:not(:disabled), [href], input:not(:disabled), select, textarea, [tabindex]:not([tabindex="-1"])'
  ) ?? [])
];

/**
 * A face (a chip, a pill) whose panel opens on hover, on keyboard focus, and
 * on a click or tap, which keeps it open. Escape, a click elsewhere, a scroll
 * that moves the face or a resize close it. The panel is portaled and placed
 * under the face, or over it where there is more room, and never taller than
 * the room it has: it is a column, and whatever in it is meant to scroll does
 * (`min-height: 0` on that part).
 *
 * Clicks and presses in the panel stop at it: through the portal they would
 * reach whatever holds the face in React's tree (a row, a title bar).
 */
export function HoverPopover({
  face,
  faceProps,
  label,
  panelClassName,
  panelStyle,
  children
}: {
  face: ReactNode;
  /** The face's own class, title and data attributes. */
  faceProps?: HTMLAttributes<HTMLSpanElement> & Record<`data-${string}`, string | undefined>;
  /** The face's accessible name. */
  label: string;
  panelClassName?: string;
  panelStyle?: CSSProperties;
  /** The panel, handed the means to close it (an action that opens a dialog). */
  children: (close: () => void) => ReactNode;
}) {
  const host = useDialogHost();
  const anchor = useRef<HTMLSpanElement>(null);
  const panel = useRef<HTMLDivElement>(null);
  const [mode, setMode] = useState<Mode | null>(null);
  const [pos, setPos] = useState<{ left: number; top: number; maxHeight: number } | null>(null);
  const timer = useRef<number | undefined>(undefined);
  // Focus given back to the face as the panel closes, which is no ask to open it.
  const returning = useRef(false);
  const open = mode !== null;

  const clearTimer = () => window.clearTimeout(timer.current);
  // A timer still pending finds it closed and leaves it so.
  const close = useCallback(() => {
    setMode(null);
    setPos(null);
  }, []);
  useEffect(() => clearTimer, []);

  const hoverIn = (e: PointerEvent) => {
    // Touch has no hover: a tap is a click.
    if (e.pointerType === 'touch') return;
    clearTimer();
    if (!open) timer.current = window.setTimeout(() => setMode(m => m ?? 'hover'), OPEN_DELAY_MS);
  };
  const hoverOut = (e: PointerEvent) => {
    if (e.pointerType === 'touch') return;
    clearTimer();
    if (mode === 'hover')
      timer.current = window.setTimeout(() => setMode(m => (m === 'hover' ? null : m)), CLOSE_GRACE_MS);
  };

  // Placed by the face's rect and the panel's measured size, again whenever
  // the panel's size changes (a tab with more rows, a helper come or gone).
  useLayoutEffect(() => {
    if (!open) return;
    // The side is chosen once: the panel's height, once capped, no longer says
    // how tall it would be, and choosing again could flip it.
    let down: boolean | null = null;
    const place = () => {
      const a = anchor.current?.getBoundingClientRect();
      const p = panel.current;
      if (!a || !p) return;
      const below = window.innerHeight - a.bottom - GAP - EDGE;
      const above = a.top - GAP - EDGE;
      const want = p.scrollHeight;
      down ??= want <= below || below >= above;
      const maxHeight = Math.max(80, down ? below : above);
      const h = Math.min(want, maxHeight);
      const left = Math.min(Math.max(EDGE, a.left - 4), window.innerWidth - p.offsetWidth - EDGE);
      setPos({ left: Math.max(EDGE, left), top: down ? a.bottom + GAP : a.top - GAP - h, maxHeight });
    };
    place();
    if (typeof ResizeObserver === 'undefined' || !panel.current) return;
    const ro = new ResizeObserver(place);
    ro.observe(panel.current);
    return () => ro.disconnect();
  }, [open, host]);

  useCloseWhenMoved(open, anchor, close);

  // A press anywhere but the face or the panel closes it; so does Escape,
  // ahead of whatever else would take it (a window's own Escape).
  useEffect(() => {
    if (!open) return;
    const away = (e: MouseEvent) => {
      const t = e.target as Node;
      if (!anchor.current?.contains(t) && !panel.current?.contains(t)) close();
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== 'Escape') return;
      e.preventDefault();
      e.stopPropagation();
      const inside = panel.current?.contains(document.activeElement);
      close();
      if (inside) {
        returning.current = true;
        anchor.current?.focus();
        returning.current = false;
      }
    };
    document.addEventListener('mousedown', away);
    window.addEventListener('keydown', onKey, true);
    return () => {
      document.removeEventListener('mousedown', away);
      window.removeEventListener('keydown', onKey, true);
    };
  }, [open, close]);

  // Focus gone from both face and panel ends what focus opened.
  const focusOut = (e: FocusEvent) => {
    const to = e.relatedTarget as Node | null;
    if (to && (anchor.current?.contains(to) || panel.current?.contains(to))) return;
    setMode(m => (m === 'focus' ? null : m));
  };

  return (
    <>
      <span
        {...faceProps}
        ref={anchor}
        role="button"
        tabIndex={0}
        aria-label={label}
        aria-haspopup="dialog"
        aria-expanded={open}
        // The bar or row holding it is draggable; a press here is not a drag.
        data-nodrag=""
        onPointerEnter={hoverIn}
        onPointerLeave={hoverOut}
        onFocus={() => {
          if (!returning.current) setMode(m => m ?? 'focus');
        }}
        onBlur={focusOut}
        onClick={e => {
          e.stopPropagation();
          clearTimer();
          // A hover or focus already showed it: the click keeps it.
          setMode(m => (m === 'pinned' ? null : 'pinned'));
        }}
        onKeyDown={e => {
          if (e.key === 'Enter' || e.key === ' ') {
            e.preventDefault();
            setMode(m => (m === 'pinned' ? null : 'pinned'));
          }
          // The panel is portaled to the end of the page: Tab goes into it
          // from here, not past everything between.
          const first = open && e.key === 'Tab' && !e.shiftKey ? focusables(panel.current)[0] : undefined;
          if (first) {
            e.preventDefault();
            first.focus();
          }
        }}
      >
        {face}
      </span>
      {open &&
        host &&
        createPortal(
          <div
            ref={panel}
            role="dialog"
            aria-label={label}
            className={`popover hover-pop${panelClassName ? ` ${panelClassName}` : ''}`}
            onClick={e => e.stopPropagation()}
            onMouseDown={e => e.stopPropagation()}
            onPointerEnter={hoverIn}
            onPointerLeave={hoverOut}
            onBlur={focusOut}
            onKeyDown={e => {
              if (e.key !== 'Tab') return;
              const all = focusables(panel.current);
              // Back out of its first control to the face; out of its last on
              // to whatever follows the face, which Tab moves to from there.
              if (e.shiftKey && document.activeElement === all[0]) {
                e.preventDefault();
                anchor.current?.focus();
              } else if (!e.shiftKey && document.activeElement === all[all.length - 1]) {
                anchor.current?.focus();
              }
            }}
            style={{
              ...panelStyle,
              left: pos?.left ?? -9999,
              top: pos?.top ?? 0,
              maxHeight: pos?.maxHeight
            }}
          >
            {children(close)}
          </div>,
          host
        )}
    </>
  );
}
