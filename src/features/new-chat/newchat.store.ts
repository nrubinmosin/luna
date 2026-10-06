import { create } from 'zustand';
import { persist } from 'zustand/middleware';
import type { Provider } from '../../shared/types';

/**
 * How a new chat's on/off choice starts out: as the last chat was made, or
 * fixed. A fixed start and remembering the last one cannot both hold, so they
 * are one setting with three values rather than two that would contradict.
 */
export type Start = 'last' | 'on' | 'off';

export const startsOn = (start: Start, last: boolean) => (start === 'last' ? last : start === 'on');

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
  lastTools: boolean;
  /** Where the two checkboxes start; set in Settings. */
  worktreeStart: Start;
  toolsStart: Start;
  openDialog: (folder?: string) => void;
  close: () => void;
  remember: (folder: string, account: { provider: Provider; name: string }, worktree: boolean, tools: boolean) => void;
  setStart: (which: 'worktreeStart' | 'toolsStart', start: Start) => void;
}

type Persisted = Pick<
  NewChatUi,
  'lastFolder' | 'lastAccount' | 'lastWorktree' | 'lastTools' | 'worktreeStart' | 'toolsStart'
>;

export const useNewChat = create<NewChatUi>()(
  persist(
    set => ({
      open: false,
      initialFolder: null,
      lastFolder: null,
      lastAccount: null,
      lastWorktree: true,
      lastTools: false,
      worktreeStart: 'last',
      // Off unless asked: a session with tools pays ~2k tokens of context for
      // them, and a plain chat should not carry that by habit.
      toolsStart: 'off',
      openDialog: folder => set({ open: true, initialFolder: folder ?? null }),
      close: () => set({ open: false, initialFolder: null }),
      remember: (folder, account, worktree, tools) =>
        set({ lastFolder: folder, lastAccount: account, lastWorktree: worktree, lastTools: tools }),
      setStart: (which, start) => set({ [which]: start })
    }),
    {
      name: 'luna.newchat',
      // Accounts gained a provider; what was stored before is a bare name.
      version: 2,
      migrate: (): Persisted => ({
        lastFolder: null,
        lastAccount: null,
        lastWorktree: true,
        lastTools: false,
        worktreeStart: 'last',
        toolsStart: 'off'
      }),
      // Whether the dialog was open is not worth restoring, and restoring it
      // would greet a cold start with a modal.
      partialize: (s): Persisted => ({
        lastFolder: s.lastFolder,
        lastAccount: s.lastAccount,
        lastWorktree: s.lastWorktree,
        lastTools: s.lastTools,
        worktreeStart: s.worktreeStart,
        toolsStart: s.toolsStart
      })
    }
  )
);
