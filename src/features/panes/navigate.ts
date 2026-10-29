import { allChats, useChats } from '../chats/chats.store';
import { currentLayout, currentSlots, usePanes } from './panes.store';
import { focusTerminal } from './terminals';

/** Puts the keyboard into the terminal of the pane at `index`, once it is laid out. */
export function focusPane(index: number) {
  const id = currentSlots(usePanes.getState())[index];
  if (id) requestAnimationFrame(() => focusTerminal(id));
}

/**
 * A click on a notification about a chat: the chat comes up on the peek
 * sheet over whatever is on screen, in the group already open — the board
 * stays as it was, and closing the sheet goes back to it.
 */
export function peekFromNotice(chatId: string) {
  const chats = useChats.getState();
  if (!chats.findChat(chatId)) return;
  chats.setActive(chatId);
  usePanes.getState().previewChat(chatId);
  requestAnimationFrame(() => focusTerminal(chatId));
}

/**
 * The next chat that is waiting for you, round the list: its group, its pane
 * if it has one on the board, the sheet if not. The status bar's "waiting"
 * and Ctrl+Shift+W both come here.
 */
export function jumpToWaiting() {
  const waiting = allChats(useChats.getState().folders).filter(c => c.status === 'waiting');
  if (!waiting.length) return;
  const active = useChats.getState().active;
  const at = waiting.findIndex(c => c.id === active);
  const chat = waiting[(at + 1) % waiting.length];
  let panes = usePanes.getState();
  if (chat.group !== panes.group) {
    panes.setGroup(chat.group);
    panes = usePanes.getState();
  }
  useChats.getState().setActive(chat.id);
  const seat = currentSlots(panes).indexOf(chat.id);
  if (seat >= 0 && seat < currentLayout(panes)) {
    panes.closePeek();
    panes.setActivePane(seat);
    focusPane(seat);
  } else {
    panes.showChat(chat.id);
    requestAnimationFrame(() => focusTerminal(chat.id));
  }
}

/** The chat whose terminal the keyboard belongs in: the one on the sheet, or
 *  the pane last worked in. */
function homeChat(): string | null | undefined {
  const s = usePanes.getState();
  if (s.peekChat) return s.peekChat;
  return currentSlots(s)[s.peek ?? s.activePane];
}

/**
 * Keeps typing going to a terminal when the keyboard ends up nowhere without
 * the user sending it there: the window comes back to the front (after an
 * update restarted it, or from another app) with nothing focused, or a
 * focused terminal is taken out of the document under the user's hands —
 * its pane remounted, its session started over. A click elsewhere, or an
 * open dialog, leaves the keyboard where it went.
 */
export function keepKeyboardHome(): () => void {
  let pointerAt = 0;
  const lost = () => !document.activeElement || document.activeElement === document.body;
  const goHome = () => {
    if (!lost() || !document.hasFocus() || document.querySelector('.scrim')) return;
    const id = homeChat();
    if (id) focusTerminal(id);
  };
  const onPointer = () => (pointerAt = performance.now());
  const onFocusOut = (e: FocusEvent) => {
    if (e.relatedTarget || performance.now() - pointerAt < 500) return;
    if (!(e.target instanceof HTMLElement) || !e.target.classList.contains('xterm-helper-textarea')) return;
    requestAnimationFrame(goHome);
  };
  const onWindowFocus = () => requestAnimationFrame(goHome);
  window.addEventListener('pointerdown', onPointer, true);
  document.addEventListener('focusout', onFocusOut);
  window.addEventListener('focus', onWindowFocus);
  // A window that opens already in front gets no focus event.
  const first = setTimeout(goHome, 1500);
  return () => {
    clearTimeout(first);
    window.removeEventListener('pointerdown', onPointer, true);
    document.removeEventListener('focusout', onFocusOut);
    window.removeEventListener('focus', onWindowFocus);
  };
}
