import { call } from './core';
import type { Provider } from '../shared/types';

export interface AccountInfo {
  provider: Provider;
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
  resetH5: string | null;
  resetWeek: string | null;
  /** Weekly limits scoped to one model, each with its own reset. */
  scoped: Array<{ label: string; used: number; resetsAt: string | null }>;
  /** 'cache' (the CLI's own, free) or 'network'. */
  source: string | null;
  fetchedAtMs: number | null;
  stale: boolean;
  /** Why an expired token could not be renewed, when that was tried. */
  refreshError: string | null;
  rateLimited: number | null;
}

export interface CodexLimitWindowDto {
  id: string;
  label: string;
  used: number;
  windowMinutes: number | null;
  resetsAt: string | null;
}

export interface CodexLimitsDto {
  signedIn: boolean;
  email: string | null;
  plan: string | null;
  haveUsage: boolean;
  windows: CodexLimitWindowDto[];
  /** 'network' or 'rollout' (the last turn's own numbers). */
  source: string | null;
  fetchedAtMs: number | null;
  stale: boolean;
  rateLimited: number | null;
}

export const listAccounts = () => call<AccountInfo[]>('list_accounts', {}, []);
/** `force` is a click on refresh: past the CLI's fresh cache and a 429 cool-off. */
export const accountLimits = (accountPath: string, force = false) =>
  call<AccountLimitsDto | null>('account_limits', { accountPath, force }, null);
export const codexLimits = (accountPath: string, force = false) =>
  call<CodexLimitsDto | null>('codex_limits', { accountPath, force }, null);

export interface AccountsRootInfo {
  path: string;
  isDefault: boolean;
}

export const getAccountsRoot = () => call<AccountsRootInfo | null>('get_accounts_root', {}, null);
/** Empty string restores the default (Documents/luna-accounts). */
export const setAccountsRoot = (path: string) => call<AccountsRootInfo>('set_accounts_root', { path });

export const createAccount = (provider: Provider, name: string) =>
  call<AccountInfo>('create_account', { provider, name });
export const deleteAccount = (provider: Provider, name: string) => call<void>('delete_account', { provider, name });

export const renameAccount = (provider: Provider, name: string, to: string) =>
  call<AccountInfo>('rename_account', { provider, name, to });
