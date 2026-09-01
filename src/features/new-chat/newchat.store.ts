import { create } from 'zustand';
import { persist } from 'zustand/middleware';

interface NewChatUi {
  open: boolean;
  initialFolder: string | null;
  /**
   * What the last chat was made with. Model, effort and permission mode are
   * deliberately not here — those come from Claude Code's own settings every
   * time, and remembering a one-off override would quietly turn it into the
   * new default. Folder, account and isolation have no settings file to come
   * from, so they are remembered instead of asked for again.
   */
  lastFolder: string | null;
  lastAccount: string | null;
  lastWorktree: boolean;
  openDialog: (folder?: string) => void;
  close: () => void;
  remember: (folder: string, account: string, worktree: boolean) => void;
}

export const useNewChat = create<NewChatUi>()(
  persist(
    set => ({
      open: false,
      initialFolder: null,
      lastFolder: null,
      lastAccount: null,
      lastWorktree: true,
      openDialog: folder => set({ open: true, initialFolder: folder ?? null }),
      close: () => set({ open: false, initialFolder: null }),
      remember: (folder, account, worktree) =>
        set({ lastFolder: folder, lastAccount: account, lastWorktree: worktree })
    }),
    {
      name: 'luna.newchat',
      // Whether the dialog was open is not worth restoring, and restoring it
      // would greet a cold start with a modal.
      partialize: s => ({
        lastFolder: s.lastFolder,
        lastAccount: s.lastAccount,
        lastWorktree: s.lastWorktree
      })
    }
  )
);
