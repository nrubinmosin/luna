import { invoke } from '@tauri-apps/api/core';
import type { Effort, ModelLabel, PermMode } from '../shared/types';
import { MODEL_CLI, PERM_CLI } from '../shared/types';

const tauriAvailable = '__TAURI_INTERNALS__' in window;

// In plain-browser dev (vite without tauri) all commands become no-ops so the
// UI stays workable.
async function call<T>(cmd: string, args?: Record<string, unknown>, fallback?: T): Promise<T> {
  if (!tauriAvailable) return fallback as T;
  return invoke<T>(cmd, args);
}

export interface AccountInfo {
  name: string;
  path: string;
}

export interface AccountLimitsDto {
  h5: number;
  week: number;
  model: number;
  resetH5: string | null;
  resetWeek: string | null;
  resetModel: string | null;
  plan: string | null;
  email: string | null;
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
  context: number | null;
}

export const sessionMeta = (id: string, accountPath: string) =>
  call<SessionMetaDto | null>('session_meta', { id, accountPath }, null);

export const removeWorktree = (folder: string, worktreePath: string) =>
  call<void>('remove_worktree', { folder, worktreePath });

export const pickFolder = async (): Promise<string | null> => {
  if (!tauriAvailable) return null;
  const { open } = await import('@tauri-apps/plugin-dialog');
  const res = await open({ directory: true, multiple: false });
  return typeof res === 'string' ? res : null;
};
