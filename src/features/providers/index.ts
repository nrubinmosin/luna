/**
 * The two providers side by side. Code that is genuinely the same for both —
 * a label, a trust write, a login window — reaches
 * them through `ui(provider)`; anything that touches a chat's settings
 * branches on `chat.provider` and imports the module it needs, because the
 * settings have nothing in common but the word.
 */
import type { Account, Provider } from '../../shared/types';
import * as claude from './claude';
import * as codex from './codex';

export { claude, codex };

export interface ProviderUi {
  label: string;
  folderTrusted: (accountPath: string, folder: string) => Promise<boolean>;
  trustFolder: (accountPath: string, folder: string) => Promise<void>;
  trustNote: string;
  loginSession: (id: string, account: Account) => Promise<string>;
  worktreeRe: RegExp;
  /** The project a session's worktree cwd belongs to, where the path says. */
  projectOf: (cwd: string) => string | null;
  /** The throwaway branch a worktree is made on; null for a detached one. */
  branchPrefix: string | null;
  /** The key the CLI takes an image from the clipboard on, as it reaches a pty. */
  pasteImageKey: string;
}

const UI: Record<Provider, ProviderUi> = { claude, codex };

export const ui = (provider: Provider): ProviderUi => UI[provider];
