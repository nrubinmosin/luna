import { useEffect } from 'react';
import { usePanes } from './panes.store';
import { useShell } from '../../app/shell.store';

type Sheet = { peek: number | null; peekChat: string | null };

const sheetOf = (): Sheet => {
  const { peek, peekChat } = usePanes.getState();
  return { peek, peekChat };
};

/** How long Alt has to be held on its own before the glance starts. */
const HOLD_MS = 150;

/**
 * Holding Alt over a row in the list brings that chat up on the sheet, and
 * letting go of Alt puts back whatever was on screen before — a look at what
 * an agent is doing without a click to go there and another to come back.
 * Moving between rows with Alt still down moves the sheet along; moving off
 * the rows keeps the last one up, so the gaps between rows don't flicker.
 *
 * A click while Alt is down keeps the sheet as it is when Alt is let go. Alt
 * pressed together with another key is a chord — Alt+digit holds a pane up,
 * Alt+V pastes an image into Claude Code, Alt+Shift switches the keyboard
 * layout — so the glance waits a beat after Alt goes down and never starts
 * if another key comes first. Starting it at once put the hovered chat on
 * the sheet under the chord: Alt+digit then saw that sheet and put it back
 * instead of raising its pane, and every chord flickered the sheet.
 *
 * The glance's sheet stands clear of the sidebar it was raised from (`aside`).
 * One kept by a click stays clear until the sidebar slides away, rather than
 * jumping under it the moment Alt is let go.
 */
export function useAltPeek() {
  useEffect(() => {
    let held = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    /** The beat has passed with Alt still alone: the glance is on. */
    let glancing = false;
    let before: Sheet | null = null;
    // What this put up last; a sheet changed by anything else is not ours to
    // put back.
    let shown: Sheet | null = null;
    let kept = false;

    const preview = (chatId: string | null) => {
      if (!chatId) return;
      usePanes.getState().previewChat(chatId);
      usePanes.getState().setAside(true);
      shown = sheetOf();
    };

    const release = () => {
      if (!held) return;
      held = false;
      clearTimeout(timer);
      glancing = false;
      const now = sheetOf();
      if (!kept && before && shown && now.peek === shown.peek && now.peekChat === shown.peekChat) {
        usePanes.setState(before);
      }
      if (!kept || !useShell.getState().overlay) usePanes.getState().setAside(false);
      before = shown = null;
    };

    const onDown = (e: KeyboardEvent) => {
      if (e.key !== 'Alt') {
        if (held) {
          kept = false;
          release();
        }
        return;
      }
      if (e.repeat || held || e.ctrlKey || e.shiftKey || e.metaKey) return;
      held = true;
      kept = false;
      before = sheetOf();
      timer = setTimeout(() => {
        glancing = true;
        preview(usePanes.getState().spot);
      }, HOLD_MS);
    };

    const onUp = (e: KeyboardEvent) => {
      if (e.key !== 'Alt' || !held) return;
      // Alt pressed and released on its own is Windows' cue for a menu bar.
      // There is none here, and a glance should not double as one either.
      if (shown) e.preventDefault();
      release();
    };

    const onMouse = () => {
      if (held) kept = true;
    };

    const unsubscribe = usePanes.subscribe((s, prev) => {
      if (glancing && s.spot && s.spot !== prev.spot) preview(s.spot);
    });
    const unsubscribeShell = useShell.subscribe((s, prev) => {
      if (!held && prev.overlay && !s.overlay) usePanes.getState().setAside(false);
    });

    window.addEventListener('keydown', onDown, true);
    window.addEventListener('keyup', onUp, true);
    window.addEventListener('mousedown', onMouse, true);
    // Alt+Tab away never delivers the keyup.
    window.addEventListener('blur', release);
    return () => {
      unsubscribe();
      unsubscribeShell();
      window.removeEventListener('keydown', onDown, true);
      window.removeEventListener('keyup', onUp, true);
      window.removeEventListener('mousedown', onMouse, true);
      window.removeEventListener('blur', release);
      clearTimeout(timer);
    };
  }, []);
}
