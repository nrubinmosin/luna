import { useEffect } from 'react';
import { numberedChats, useChats } from '../features/chats/chats.store';
import { quickChat } from '../features/new-chat/create';
import { currentLayout, usePanes, type Layout } from '../features/panes/panes.store';
import { focusPane, jumpToWaiting } from '../features/panes/navigate';
import { useShell } from './shell.store';
import { useShortcuts } from '../features/shortcuts/ShortcutsDialog';

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
 * downstream could have used it anyway. Nor has it one for Ctrl+Shift+<letter>
 * apart from Ctrl+<letter>, so Ctrl+Shift+B and Ctrl+Shift+W take nothing from
 * the CLIs — Ctrl+B and Ctrl+W themselves travel on. Ctrl+Tab (without Shift, which Claude
 * Code binds) and F1 are taken always: the CLIs bind neither. Ctrl+arrows were the obvious
 * pick for moving between panes and are left alone — they move by word in the
 * CLI's prompt. Escape is deliberately left alone —
 * it is how the CLI is interrupted, and no view of Luna's is worth eating it.
 */
export function useKeymap(opts: { openNewChat: () => void; closeModal: () => void }) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        // An open account list closes first, on its own.
        if (document.querySelector('[data-picker-open]')) return;
        opts.closeModal();
        return;
      }
      if (e.key === 'F1' && !e.ctrlKey && !e.altKey && !e.shiftKey && !e.metaKey) {
        e.preventDefault();
        e.stopPropagation();
        const s = useShortcuts.getState();
        s.setOpen(!s.open);
        return;
      }
      // Alt+1…4 holds that pane up on the sheet, and again puts it back. Plain
      // Alt only: AltGr arrives as Ctrl+Alt and types characters on some
      // layouts. The CLIs bind no Alt+digit.
      if (e.altKey && !e.ctrlKey && !e.metaKey && !e.shiftKey) {
        const digit = digitOf(e);
        const panes = usePanes.getState();
        if (digit == null || digit < 1 || digit > currentLayout(panes) || currentLayout(panes) < 2) return;
        e.preventDefault();
        e.stopPropagation();
        const i = digit - 1;
        if (panes.peek === i) panes.closePeek();
        else {
          panes.setActivePane(i);
          panes.setPeek(i);
        }
        focusPane(i);
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

      // Ctrl+Shift+B pins the sidebar or lets it slide away; Ctrl+Shift+W
      // goes to the next chat that is waiting for you. By physical key, so a
      // Cyrillic layout gets them too.
      if (e.shiftKey && e.code === 'KeyB') {
        take();
        const shell = useShell.getState();
        shell.setPinned(!shell.pinned);
        return;
      }
      if (e.shiftKey && e.code === 'KeyW') {
        take();
        jumpToWaiting();
        return;
      }

      // Ctrl+Tab walks the groups round. Ctrl+Shift+Tab is Claude Code's
      // own and travels on to the terminal untouched.
      if (e.key === 'Tab') {
        if (e.shiftKey) return;
        take();
        usePanes.getState().nextGroup();
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

      // With panes to choose between, a digit walks the keyboard into that
      // pane (Alt+digit, above, holds it up instead). With only one pane the
      // digits count through the list — the same numbers the rows are wearing.
      if (layout > 1) {
        if (digit > layout) return;
        take();
        const i = digit - 1;
        panes.closePeek();
        panes.setActivePane(i);
        focusPane(i);
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
