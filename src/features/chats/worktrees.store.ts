import { useEffect } from 'react';
import { create } from 'zustand';
import { orphanWorktrees } from '../../ipc/commands';
import { useAccounts } from '../accounts/accounts.store';
import { allChats, useChats } from './chats.store';

/** A worktree a CLI made for `folder` that no chat or running session uses. */
export interface StaleWorktree {
  folder: string;
  path: string;
}

/** Claude Code's live under the folder; anything else found is Codex's. */
export const isClaudeWorktree = (path: string) => /[\\/]\.claude[\\/]worktrees[\\/]/i.test(path);

/**
 * What a scan or a sweep is checked against: every folder and every chat's
 * worktree, all groups' — the board shows one group, and judging by it would
 * offer up the worktrees of chats parked in the other three.
 */
export function scanScope() {
  const folders = useChats.getState().folders;
  return {
    folders: [...new Set(folders.map(f => f.path))],
    inUse: allChats(folders)
      .map(c => c.worktreePath)
      .filter((p): p is string => !!p),
    accountPaths: useAccounts.getState().accounts.map(a => a.path)
  };
}

interface WorktreesState {
  stale: StaleWorktree[];
  /** The Worktrees window on the title bar. */
  open: boolean;
  setOpen: (v: boolean) => void;
  rescan: () => Promise<void>;
}

export const useWorktrees = create<WorktreesState>(set => ({
  stale: [],
  open: false,
  setOpen: v => set({ open: v }),
  rescan: async () => {
    const { folders, inUse, accountPaths } = scanScope();
    const found = await Promise.all(
      folders.map(folder =>
        orphanWorktrees(folder, inUse, accountPaths)
          .catch(() => [] as string[])
          .then(paths => paths.map(path => ({ folder, path })))
      )
    );
    set({ stale: found.flat() });
  }
}));

/**
 * Keeps the count on the title bar's Worktrees button current: every minute,
 * and whenever a chat's worktree or the list of folders changes — that is
 * when one is most likely to have just been made or let go.
 */
export function useWorktreeScan() {
  const key = useChats(s =>
    s.folders.map(f => f.path).join('|') + '#' + allChats(s.folders).map(c => c.worktreePath ?? '').join('|')
  );
  useEffect(() => {
    const rescan = () => void useWorktrees.getState().rescan();
    rescan();
    const t = setInterval(rescan, 60_000);
    return () => clearInterval(t);
  }, [key]);
}
