import { useEffect } from 'react';
import { usePanes } from './panes.store';

type Sheet = { peek: number | null; peekChat: string | null };

const sheetOf = (): Sheet => {
  const { peek, peekChat } = usePanes.getState();
  return { peek, peekChat };
};

/**
 * Holding Alt over a row in the list brings that chat up on the sheet, and
 * letting go of Alt puts back whatever was on screen before — a look at what
 * an agent is doing without a click to go there and another to come back.
 * Moving between rows with Alt still down moves the sheet along; moving off
 * the rows keeps the last one up, so the gaps between rows don't flicker.
 *
 * A click while Alt is down keeps the sheet as it is when Alt is let go. Alt
 * pressed together with another key is a chord meant for the terminal — Alt
 * alone sends it nothing — so the glance is called off and nothing is taken.
 */
export function useAltPeek() {
  useEffect(() => {
    let held = false;
    let before: Sheet | null = null;
    // What this put up last; a sheet changed by anything else is not ours to
    // put back.
    let shown: Sheet | null = null;
    let kept = false;

    const preview = (chatId: string | null) => {
      if (!chatId) return;
      usePanes.getState().previewChat(chatId);
      shown = sheetOf();
    };

    const release = () => {
      if (!held) return;
      held = false;
      const now = sheetOf();
      if (!kept && before && shown && now.peek === shown.peek && now.peekChat === shown.peekChat) {
        usePanes.setState(before);
      }
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
      preview(usePanes.getState().spot);
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
      if (held && s.spot && s.spot !== prev.spot) preview(s.spot);
    });

    window.addEventListener('keydown', onDown, true);
    window.addEventListener('keyup', onUp, true);
    window.addEventListener('mousedown', onMouse, true);
    // Alt+Tab away never delivers the keyup.
    window.addEventListener('blur', release);
    return () => {
      unsubscribe();
      window.removeEventListener('keydown', onDown, true);
      window.removeEventListener('keyup', onUp, true);
      window.removeEventListener('mousedown', onMouse, true);
      window.removeEventListener('blur', release);
    };
  }, []);
}
