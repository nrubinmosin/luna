import { invoke } from '@tauri-apps/api/core';
import type { Effort, ModelLabel, PermMode } from '../shared/types';
import { MODEL_CLI, PERM_CLI } from '../shared/types';
import { logWarn, SLOW_MS } from '../shared/lib/log';

const tauriAvailable = '__TAURI_INTERNALS__' in window;

// In plain-browser dev (vite without tauri) all commands become no-ops so the
// UI stays workable.
async function call<T>(cmd: string, args?: Record<string, unknown>, fallback?: T): Promise<T> {
  if (!tauriAvailable) return fallback as T;
  // Timed because a sync Tauri command runs on the main thread: a slow one is
  // indistinguishable from the app hanging, and this names which one it was.
  const started = performance.now();
  try {
    return await invoke<T>(cmd, args);
  } catch (e) {
    logWarn('ipc', `${cmd} failed: ${String(e)}`);
    throw e;
  } finally {
    const ms = performance.now() - started;
    if (ms > SLOW_MS) logWarn('ipc', `${cmd} took ${Math.round(ms)}ms`);
  }
}

export interface AccountInfo {
  name: string;
  path: string;
}

export interface AccountLimitsDto {
  /** Refresh token present and unexpired — the real "is this account usable". */
  signedIn: boolean;
  email: string | null;
  plan: string | null;
  /** False when no usage numbers are known; show "—", not 0%. */
  haveUsage: boolean;
  h5: number;
  week: number;
  model: number;
  resetH5: string | null;
  resetWeek: string | null;
  resetModel: string | null;
  /** 'cache' (the CLI's own, free) or 'network'. */
  source: string | null;
  fetchedAtMs: number | null;
  stale: boolean;
  rateLimited: number | null;
}

export const listAccounts = () => call<AccountInfo[]>('list_accounts', {}, []);
export const accountLimits = (accountPath: string) =>
  call<AccountLimitsDto | null>('account_limits', { accountPath }, null);
export const createAccount = (name: string) => call<AccountInfo>('create_account', { name });
export const deleteAccount = (name: string) => call<void>('delete_account', { name });

export interface SessionSpec {
  chatId: string;
  folder: string;
  accountPath: string;
  model: ModelLabel;
  effort: Effort;
  perm: PermMode;
  worktree: boolean;
  resume?: string | null;
}

export const ensureSession = (spec: SessionSpec) =>
  call<string>('ensure_session', {
    id: spec.chatId,
    folder: spec.folder,
    accountPath: spec.accountPath,
    // Fallback covers chats persisted before the model labels changed.
    model: MODEL_CLI[spec.model] ?? spec.model.toLowerCase().split(' ')[0],
    effort: spec.effort,
    permissionMode: PERM_CLI[spec.perm],
    worktree: spec.worktree,
    resume: spec.resume ?? null
  }, '');

export const writeSession = (id: string, data: string) =>
  call<void>('write_session', { id, data });

export const resizeSession = (id: string, cols: number, rows: number) =>
  call<void>('resize_session', { id, cols, rows });

export const killSession = (id: string) => call<void>('kill_session', { id });

export const sessionAlive = (id: string) => call<boolean>('session_alive', { id }, false);

export interface SessionMetaDto {
  name: string | null;
  status: string | null;
  cwd: string | null;
  sessionId: string | null;
  nameSource: string | null;
  context: number | null;
  contextTokens: number | null;
  contextWindow: number | null;
  firstPrompt: string | null;
}

export const sessionMeta = (id: string, accountPath: string) =>
  call<SessionMetaDto | null>('session_meta', { id, accountPath }, null);

export const removeWorktree = (folder: string, worktreePath: string) =>
  call<void>('remove_worktree', { folder, worktreePath });

/**
 * Kills the session and cleans up after it in one call: the chat's attachments
 * always, its worktree and branch only when asked. Resolves the worktree
 * itself, so it works even when the UI never saw the path. Returns that
 * worktree whether or not it was removed.
 */
export const deleteSession = (
  chatId: string,
  folder: string,
  accountPath: string,
  worktreePath: string | null,
  dropWorktree: boolean
) =>
  call<string | null>(
    'delete_session',
    { id: chatId, folder, accountPath, worktreePath, dropWorktree },
    null
  );

/**
 * Worktree dirs under the folder that no live chat is using. `accountPaths`
 * lets the backend also spare any cwd a running CLI session sits in, which
 * covers sessions whose path the UI has not learned yet.
 */
export const orphanWorktrees = (folder: string, inUse: string[], accountPaths: string[]) =>
  call<string[]>('orphan_worktrees', { folder, inUse, accountPaths }, []);

export const removeOrphanWorktrees = (folder: string, inUse: string[], accountPaths: string[]) =>
  call<number>('remove_orphan_worktrees', { folder, inUse, accountPaths }, 0);

/** Whether this account already accepted Claude Code's trust prompt for the folder. */
export const folderTrusted = (accountPath: string, folder: string) =>
  call<boolean>('folder_trusted', { accountPath, folder }, true);

/** Writes the same trust bit the CLI's own prompt would write. */
export const trustFolder = (accountPath: string, folder: string) =>
  call<void>('trust_folder', { accountPath, folder });

/** Copies a pasted/dropped file into the app's media store, returns its absolute path. */
export const saveMedia = (chatId: string, name: string, base64: string) =>
  call<string | null>('save_media', { chatId, name, data: base64 }, null);

export const clearMedia = (chatId: string) => call<void>('clear_media', { chatId });

export const pickFolder = async (): Promise<string | null> => {
  if (!tauriAvailable) return null;
  const { open } = await import('@tauri-apps/plugin-dialog');
  const res = await open({ directory: true, multiple: false });
  return typeof res === 'string' ? res : null;
};
