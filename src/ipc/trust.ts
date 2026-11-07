import { call } from './core';

/** Whether this account already accepted Claude Code's trust prompt for the folder. */
export const claudeFolderTrusted = (accountPath: string, folder: string) =>
  call<boolean>('claude_folder_trusted', { accountPath, folder }, true);

/** Writes the same trust bit the CLI's own prompt would write. */
export const claudeTrustFolder = (accountPath: string, folder: string) =>
  call<void>('claude_trust_folder', { accountPath, folder });

export const codexFolderTrusted = (accountPath: string, folder: string) =>
  call<boolean>('codex_folder_trusted', { accountPath, folder }, true);

export const codexTrustFolder = (accountPath: string, folder: string) =>
  call<void>('codex_trust_folder', { accountPath, folder });

/** Whether Codex runs a folder's hooks without asking: the default in
 *  Settings and what was kept for the folder (codex/hooks.rs). */
export interface CodexHooksStart {
  default: boolean;
  kept: boolean | null;
}
export const codexHooksStart = (folder: string) =>
  call<CodexHooksStart>('codex_hooks_start', { folder }, { default: false, kept: null });
export const setCodexHooksDefault = (on: boolean) => call<boolean>('set_codex_hooks_default', { on }, on);
/** Folders whose projects Codex sessions start trusted on this machine
 *  (codex/trust.rs). */
export const codexTrustedRoots = () => call<string[]>('codex_trusted_roots', {}, []);
export const setCodexTrustedRoots = (roots: string[]) => call<string[]>('set_codex_trusted_roots', { roots }, roots);
/** null hands the folder back to the default. */
export const keepCodexHooks = (folder: string, on: boolean | null) =>
  call<void>('keep_codex_hooks', { folder, on }, undefined);
