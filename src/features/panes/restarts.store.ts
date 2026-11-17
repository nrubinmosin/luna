import { create } from 'zustand';

/**
 * How many times each chat's CLI was started again under its pane
 * (terminals.ts `resumeSession`), and which chats wait for that: a pane showing
 * one makes its terminal afresh on the new process, and a box that asked for it
 * can say it is still to come. Not persisted: a new window's terminals are new.
 */
interface Restarts {
  by: Record<string, number>;
  /** Chats whose CLI is to start again once they are idle. */
  pending: Record<string, true>;
  bump: (chatId: string) => void;
  setPending: (chatId: string, on: boolean) => void;
  /** The chat is gone: nothing more to count for it. */
  forgetChat: (chatId: string) => void;
}

export const useRestarts = create<Restarts>()(set => ({
  by: {},
  pending: {},
  bump: chatId => set(s => ({ by: { ...s.by, [chatId]: (s.by[chatId] ?? 0) + 1 } })),
  setPending: (chatId, on) =>
    set(s => {
      const pending = { ...s.pending };
      if (on) pending[chatId] = true;
      else delete pending[chatId];
      return { pending };
    }),
  forgetChat: chatId =>
    set(s => {
      if (!(chatId in s.by) && !(chatId in s.pending)) return s;
      const { [chatId]: _b, ...by } = s.by;
      const { [chatId]: _p, ...pending } = s.pending;
      return { by, pending };
    })
}));
