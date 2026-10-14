import { allChats, useChats } from '../chats/chats.store';
import { currentLayout, currentSlots, usePanes } from './panes.store';
import { focusTerminal } from './terminals';

/** Puts the keyboard into the terminal of the pane at `index`, once it is laid out. */
export function focusPane(index: number) {
  const id = currentSlots(usePanes.getState())[index];
  if (id) requestAnimationFrame(() => focusTerminal(id));
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
