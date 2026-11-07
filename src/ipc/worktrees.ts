import { call } from './core';

/** Whether origin is fetched before a worktree is made (Settings). */
export const fetchBeforeWorktree = () => call<boolean>('fetch_before_worktree', {}, true);
export const setFetchBeforeWorktree = (on: boolean) => call<boolean>('set_fetch_before_worktree', { on }, on);

/**
 * Kills the session and cleans up after it in one call: the chat's attachments
 * always, its worktree and branch only when asked. Resolves the worktree
 * itself, so it works even when the UI never saw the path. Returns that
 * worktree whether or not it was removed.
 */
export const deleteSession = (chatId: string, folder: string, worktreePath: string | null, dropWorktree: boolean) =>
  call<string | null>('delete_session', { id: chatId, folder, worktreePath, dropWorktree }, null);

/**
 * Worktree dirs under the folder that no live chat is using. `accountPaths`
 * lets the backend also spare any cwd a running CLI session sits in, which
 * covers sessions whose path the UI has not learned yet.
 */
export const orphanWorktrees = (folder: string, inUse: string[], accountPaths: string[]) =>
  call<string[]>('orphan_worktrees', { folder, inUse, accountPaths }, []);

/** What deleting a worktree would throw away. */
export interface WorktreeInfoDto {
  path: string;
  branch: string | null;
  /** Entries `git status` lists, untracked included. */
  uncommitted: number | null;
  /** Commits no other branch, local or remote, has. */
  uniqueCommits: number | null;
  touchedMs: number | null;
  /** Git cannot read it as a checkout; nothing could be counted. */
  broken: boolean;
}

export const inspectWorktrees = (folder: string, paths: string[]) =>
  call<WorktreeInfoDto[]>('inspect_worktrees', { folder, paths }, []);

export interface SweepResultDto {
  removed: string[];
  failed: { path: string; error: string }[];
}

/** Removes the picked worktrees that are still orphans when the call lands. */
export const removeOrphanWorktrees = (folder: string, inUse: string[], accountPaths: string[], paths: string[]) =>
  call<SweepResultDto>('remove_orphan_worktrees', { folder, inUse, accountPaths, paths }, { removed: [], failed: [] });
