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
