import { create } from 'zustand';
import { persist } from 'zustand/middleware';
import type { Folder, FolderStart, Provider } from '../../shared/types';

/**
 * Both checkboxes for a new chat in `folder`: the folder's own start if one
 * was kept for it, else the defaults from Settings. Nothing a chat is made
 * with changes either — a box flipped for one quick chat is not a new habit.
 */
export function startFor(folder: Folder | undefined): FolderStart {
  const ui = useNewChat.getState();
  return folder?.start ?? { worktree: ui.worktree, tools: ui.tools };
}

interface NewChatUi {
  open: boolean;
  initialFolder: string | null;
  /**
   * Where the last chat was made. Model, effort and the permission or sandbox
   * modes are deliberately not here — those come from the CLI's own settings
   * every time, and remembering a one-off override would quietly turn it into
   * the new default. Folder and account have no settings file to come from,
   * so they are remembered instead of asked for again.
   */
  lastFolder: string | null;
  lastAccount: { provider: Provider; name: string } | null;
  /** How the two checkboxes start in a folder without a start of its own; set in Settings. */
  worktree: boolean;
  tools: boolean;
  openDialog: (folder?: string) => void;
  close: () => void;
  remember: (folder: string, account: { provider: Provider; name: string }) => void;
  setDefault: (which: 'worktree' | 'tools', on: boolean) => void;
}

type Persisted = Pick<NewChatUi, 'lastFolder' | 'lastAccount' | 'worktree' | 'tools'>;

/** What version 2 stored: each box started "as last time" or fixed. */
type V2 = {
  lastFolder?: string | null;
  lastAccount?: NewChatUi['lastAccount'];
  lastWorktree?: boolean;
  lastTools?: boolean;
  worktreeStart?: 'last' | 'on' | 'off';
  toolsStart?: 'last' | 'on' | 'off';
};

export const useNewChat = create<NewChatUi>()(
  persist(
    set => ({
      open: false,
      initialFolder: null,
      lastFolder: null,
      lastAccount: null,
      worktree: true,
      // Off unless asked: a session with tools pays ~2k tokens of context for
      // them, and a plain chat should not carry that by habit.
      tools: false,
      openDialog: folder => set({ open: true, initialFolder: folder ?? null }),
      close: () => set({ open: false, initialFolder: null }),
      remember: (folder, account) => set({ lastFolder: folder, lastAccount: account }),
      setDefault: (which, on) => set({ [which]: on })
    }),
    {
      name: 'luna.newchat',
      // 2: accounts gained a provider. 3: the boxes start from fixed defaults
      // instead of following the last chat made.
      version: 3,
      migrate: (persisted, version): Persisted => {
        if (version < 2) return { lastFolder: null, lastAccount: null, worktree: true, tools: false };
        // A box that started "as last time" keeps what it would have shown.
        const p = (persisted ?? {}) as V2;
        const fixed = (start: V2['worktreeStart'], last: boolean | undefined, otherwise: boolean) =>
          start === 'on' ? true : start === 'off' ? false : last ?? otherwise;
        return {
          lastFolder: p.lastFolder ?? null,
          lastAccount: p.lastAccount ?? null,
          worktree: fixed(p.worktreeStart, p.lastWorktree, true),
          tools: fixed(p.toolsStart, p.lastTools, false)
        };
      },
      // Whether the dialog was open is not worth restoring, and restoring it
      // would greet a cold start with a modal.
      partialize: (s): Persisted => ({
        lastFolder: s.lastFolder,
        lastAccount: s.lastAccount,
        worktree: s.worktree,
        tools: s.tools
      })
    }
  )
);
