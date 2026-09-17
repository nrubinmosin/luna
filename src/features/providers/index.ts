/**
 * The two providers side by side. Code that is genuinely the same for both —
 * a label, a trust write, a login window, the limit bars of a row — reaches
 * them through `ui(provider)`; anything that touches a chat's settings
 * branches on `chat.provider` and imports the module it needs, because the
 * settings have nothing in common but the word.
 */
import type { CSSProperties, ComponentType } from 'react';
import type { Account, Provider } from '../../shared/types';
import * as claude from './claude';
import * as codex from './codex';

export { claude, codex };

export interface ProviderUi {
  label: string;
  vendor: string;
  folderTrusted: (accountPath: string, folder: string) => Promise<boolean>;
  trustFolder: (accountPath: string, folder: string) => Promise<void>;
  trustNote: string;
  loginSession: (id: string, account: Account) => Promise<string>;
  worktreeRe: RegExp;
  worktreeParentRe: RegExp;
  branchPrefix: string;
  LimitBars: ComponentType<{ account: Account }>;
  worstLimit: (account: Account) => number;
  longResetAt: (account: Account) => string | null;
}

const UI: Record<Provider, ProviderUi> = { claude, codex };

export const ui = (provider: Provider): ProviderUi => UI[provider];

/** The chip styles a pane hands its provider's `Chips`. */
export type ChipStyles = { chip: CSSProperties; softChip: CSSProperties };
