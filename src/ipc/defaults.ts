import { call } from './core';

/** Where a resolved default came from, for the line under the control;
 *  `stock` is Luna's own choice where no file said anything usable. */
export type ClaudeDefaultSource = 'account' | 'project' | 'project-local' | 'managed' | 'stock';
/** `codex`: what Codex itself would pick — the first model of its catalog,
 *  at that model's own reasoning level. */
export type CodexDefaultSource = 'account' | 'project' | 'codex' | 'stock';

export interface SettingDto<S extends string = string> {
  value: string;
  source: S;
}

export interface ClaudeDefaultsDto {
  model: SettingDto<ClaudeDefaultSource> | null;
  effort: SettingDto<ClaudeDefaultSource> | null;
  permissionMode: SettingDto<ClaudeDefaultSource> | null;
}

/**
 * Model, effort and permission mode as Claude Code itself would resolve them
 * for this account in this folder, already settled (stock included) by the
 * core. Null only when the core could not be asked.
 */
export const claudeDefaults = (accountPath: string, folder: string) =>
  call<ClaudeDefaultsDto>(
    'claude_defaults',
    { accountPath, folder },
    { model: null, effort: null, permissionMode: null }
  );

export interface CodexDefaultsDto {
  model: SettingDto<CodexDefaultSource> | null;
  effort: SettingDto<CodexDefaultSource> | null;
  approval: SettingDto<CodexDefaultSource> | null;
  sandbox: SettingDto<CodexDefaultSource> | null;
}

/** The same out of Codex's `config.toml` — the account's, then the project's
 *  (which Codex only reads once the folder is trusted). */
export const codexDefaults = (accountPath: string, folder: string) =>
  call<CodexDefaultsDto>(
    'codex_defaults',
    { accountPath, folder },
    { model: null, effort: null, approval: null, sandbox: null }
  );

export interface CodexModelDto {
  slug: string;
  displayName: string;
  defaultEffort: string | null;
  efforts: string[];
  /** What Codex says each level is for. */
  effortHints: Record<string, string>;
}

/** The models Codex lists for this account, off its own cache, in Codex's
 *  order (the first is its default) — with the reasoning levels each one
 *  takes. Empty until the CLI has run once. */
export const codexModels = (accountPath: string) => call<CodexModelDto[]>('codex_models', { accountPath }, []);
