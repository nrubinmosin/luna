import { create } from 'zustand';
import { persist } from 'zustand/middleware';
import type { Provider } from '../../shared/types';

interface NewChatUi {
  open: boolean;
  initialFolder: string | null;
  /**
   * What the last chat was made with. Model, effort and the permission or
   * sandbox modes are deliberately not here — those come from the CLI's own
   * settings every time, and remembering a one-off override would quietly
   * turn it into the new default. Folder, account and isolation have no
   * settings file to come from, so they are remembered instead of asked for
   * again.
   */
  lastFolder: string | null;
  lastAccount: { provider: Provider; name: string } | null;
  lastWorktree: boolean;
  openDialog: (folder?: string) => void;
  close: () => void;
  remember: (folder: string, account: { provider: Provider; name: string }, worktree: boolean) => void;
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
      // Accounts gained a provider; what was stored before is a bare name.
      version: 2,
      migrate: () => ({ lastFolder: null, lastAccount: null, lastWorktree: true }),
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
