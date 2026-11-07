import { create } from 'zustand';
import { dismissWaiting } from '../../ipc/worktrees';
import { useChats } from './chats.store';

/**
 * The chats whose `waiting` comes from the marker their last message ends on
 * (`SessionMetaDto.marked`), filled by useSessionWatch: the ones a person may
 * clear by hand, which a CLI's own prompt is not, each by the key of the
 * ending it was shown on. Not kept with the chats: it is about the turn the
 * session ended last.
 */
export const useMarked = create<{
  by: Record<string, string>;
  set: (chatId: string, key: string | null) => void;
}>(set => ({
  by: {},
  set: (chatId, key) =>
    set(s => {
      if ((s.by[chatId] ?? null) === key) return s;
      const by = { ...s.by };
      if (key) by[chatId] = key;
      else delete by[chatId];
      return { by };
    })
}));

export const CLEAR_WAITING_TITLE = 'Not waiting — clear until a turn ends asking again';

/** The chat is not waiting on you after all: the core keeps it cleared until
 *  a turn ends on the marker again, and the row and the pane rest now rather
 *  than at the watcher's next look. */
export function clearWaiting(chatId: string) {
  const key = useMarked.getState().by[chatId];
  if (!key) return;
  useMarked.getState().set(chatId, null);
  useChats.getState().setStatus(chatId, 'resting');
  // Only the ending shown here: one a later look found stands. A core that
  // is not there has no waiting to clear either.
  void dismissWaiting(chatId, key).catch(() => {});
}
