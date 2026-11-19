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

/** Luna starts no Codex hooks any more and the commands behind these three
 *  are gone from the core; the window that still asks is told "nothing to
 *  start" here instead of getting a rejection. */
export interface CodexHooksStart {
  default: boolean;
  kept: boolean | null;
}
/* eslint-disable @typescript-eslint/no-unused-vars -- the arguments keep the callers' shape */
export const codexHooksStart = (_folder: string): Promise<CodexHooksStart> =>
  Promise.resolve({ default: false, kept: null });
export const setCodexHooksDefault = (_on: boolean): Promise<boolean> => Promise.resolve(false);
/** Folders whose projects Codex sessions start trusted on this machine
 *  (codex/trust.rs). */
export const codexTrustedRoots = () => call<string[]>('codex_trusted_roots', {}, []);
export const setCodexTrustedRoots = (roots: string[]) => call<string[]>('set_codex_trusted_roots', { roots }, roots);
export const keepCodexHooks = (_folder: string, _on: boolean | null): Promise<void> => Promise.resolve();
/* eslint-enable @typescript-eslint/no-unused-vars */
