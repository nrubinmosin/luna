import { useEffect } from 'react';
import { numberedChats, useChats } from '../features/chats/chats.store';
import { quickChat } from '../features/new-chat/create';
import { currentLayout, usePanes, type Layout } from '../features/panes/panes.store';

/**
 * Layout-independent digit, so the shortcuts survive a Cyrillic keyboard and
 * the numeric keypad alike. `code` is the physical key; `key` is the fallback
 * for anything that does not report one.
 */
function digitOf(e: KeyboardEvent): number | null {
  const physical = /^(?:Digit|Numpad)(\d)$/.exec(e.code);
  if (physical) return Number(physical[1]);
  return /^\d$/.test(e.key) ? Number(e.key) : null;
}

/**
 * The chords Luna keeps for itself.
 *
 * Listening in the capture phase is the whole trick. xterm cancels the keys it
 * handles — `preventDefault` *and* `stopPropagation` — so a listener bound the
 * ordinary way never hears a keystroke while a terminal has focus, which is
 * nearly always. Capture runs on the way down, before the terminal sees
 * anything, so what Luna takes here it takes for good and everything else
 * travels on untouched.
 *
 * Ctrl+<digit> is safe to take: a terminal has no encoding for it, so nothing
 * downstream could have used it anyway. Escape is deliberately left alone —
 * it is how the CLI is interrupted, and no view of Luna's is worth eating it.
 */
export function useKeymap(opts: { openNewChat: () => void; closeModal: () => void }) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        opts.closeModal();
        return;
      }
      const mod = e.metaKey || e.ctrlKey;
      if (!mod || e.altKey) return;

      /** Claim the keystroke: the terminal underneath must not also get it. */
      const take = () => {
        e.preventDefault();
        e.stopPropagation();
      };

      if (e.code === 'KeyN' || e.key.toLowerCase() === 'n') {
        take();
        if (e.shiftKey) void quickChat();
        else opts.openNewChat();
        return;
      }

      const digit = digitOf(e);
      if (digit == null) return;
      const panes = usePanes.getState();
      const layout = currentLayout(panes);

      // Shift is the arrangement, plain is the view: the layout is chosen once
      // in a while, panes are moved between constantly.
      if (e.shiftKey) {
        if (digit >= 1 && digit <= 4) {
          take();
          panes.setLayout(digit as Layout);
        }
        return;
      }

      if (digit === 0) {
        take();
        panes.closePeek();
        return;
      }

      // With panes to choose between, a digit holds that pane up on the sheet
      // and pressing it again puts it back. With only one pane there is nothing
      // to hold up, so the digits count through the list instead — the same
      // numbers the rows are wearing.
      if (layout > 1) {
        if (digit > layout) return;
        take();
        panes.setPeek(panes.peek === digit - 1 ? null : digit - 1);
        return;
      }

      const chat = numberedChats(useChats.getState().folders, panes.group)[digit - 1];
      if (!chat) return;
      take();
      useChats.getState().setActive(chat.id);
      panes.showChat(chat.id);
    };
    window.addEventListener('keydown', onKey, true);
    return () => window.removeEventListener('keydown', onKey, true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
}
