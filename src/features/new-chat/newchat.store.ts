import { create } from 'zustand';
import { persist } from 'zustand/middleware';
import type { Folder, FolderStart, Provider } from '../../shared/types';

/** How the new-chat checkboxes start: every one of them said. */
export type Start = Required<FolderStart>;

/**
 * The boxes as `own`, a folder's kept start, has them, else as `defaults`.
 * A start kept before the wait guard was part of it says nothing of the
 * guard, which then follows the default.
 */
export const startOf = (own: FolderStart | undefined, defaults: Start): Start =>
  own ? { ...own, guard: own.guard ?? defaults.guard } : defaults;

/**
 * The checkboxes for a new chat in `folder`: the folder's own start if one
 * was kept for it, else the defaults from Settings. Nothing a chat is made
 * with changes either — a box flipped for one quick chat is not a new habit.
 */
export function startFor(folder: Folder | undefined): Start {
  const ui = useNewChat.getState();
  return startOf(folder?.start, { worktree: ui.worktree, tools: ui.tools, guard: ui.guard });
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
  /** How the checkboxes start in a folder without a start of its own; set in Settings. */
  worktree: boolean;
  tools: boolean;
  guard: boolean;
  openDialog: (folder?: string) => void;
  close: () => void;
  remember: (folder: string, account: { provider: Provider; name: string }) => void;
  setDefault: (which: keyof Start, on: boolean) => void;
}

type Persisted = Pick<NewChatUi, 'lastFolder' | 'lastAccount' | 'worktree' | 'tools' | 'guard'>;

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
      // Off unless asked: a chat's CLI runs every command as it would on its
      // own. A stored state from before this default was offered has none,
      // and takes this one.
      guard: false,
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
        if (version < 2) return { lastFolder: null, lastAccount: null, worktree: true, tools: false, guard: false };
        // A box that started "as last time" keeps what it would have shown.
        const p = (persisted ?? {}) as V2;
        const fixed = (start: V2['worktreeStart'], last: boolean | undefined, otherwise: boolean) =>
          start === 'on' ? true : start === 'off' ? false : (last ?? otherwise);
        return {
          lastFolder: p.lastFolder ?? null,
          lastAccount: p.lastAccount ?? null,
          worktree: fixed(p.worktreeStart, p.lastWorktree, true),
          tools: fixed(p.toolsStart, p.lastTools, false),
          guard: false
        };
      },
      // Whether the dialog was open is not worth restoring, and restoring it
      // would greet a cold start with a modal.
      partialize: (s): Persisted => ({
        lastFolder: s.lastFolder,
        lastAccount: s.lastAccount,
        worktree: s.worktree,
        tools: s.tools,
        guard: s.guard
      })
    }
  )
);
