import { invoke } from '@tauri-apps/api/core';
import type { CodexApproval, CodexEffort, CodexSandbox, Effort, ModelLabel, PermMode, Provider } from '../shared/types';
import { MODEL_CLI, PERM_CLI } from '../shared/types';
import { logWarn, SLOW_MS } from '../shared/lib/log';

const tauriAvailable = '__TAURI_INTERNALS__' in window;

// Commands that do their real work off the main thread. A slow one means a slow
// network or a big transcript — not a frozen window — so they get a budget of
// their own rather than tripping the stall warning thousands of times a day.
const SLOW_BUDGET_MS: Record<string, number> = {
  // A usage request may wait out a 10s read timeout, and a token renewal before it.
  account_limits: 15_000,
  codex_limits: 15_000,
  session_meta: 2000,
  saved_title: 5000,
  ensure_claude_session: 30_000,
  ensure_codex_session: 30_000,
  // Both give the CLI a moment to quit on its own before killing it; deleting
  // then waits for the worktree to come free.
  kill_session: 5000,
  delete_session: 20_000,
  orphan_worktrees: 10_000,
  remove_orphan_worktrees: 30_000
};

// What the core answers (corecmd.rs): everything that touches the running
// sessions, which live in the core process rather than the window's. The
// window passes them on; keystrokes and resizes go without waiting for an
// answer, in the order they are made.
const CORE_CALL: ReadonlySet<string> = new Set([
  'ensure_claude_session', 'ensure_codex_session', 'kill_session', 'delete_session', 'stop_all_sessions',
  'orphan_sessions', 'session_meta', 'session_modes', 'agent_spawned', 'chat_names', 'account_usage', 'power_state',
  'background_helpers', 'background_promote', 'background_kill', 'background_delete',
  'set_keep_awake', 'arm_power_off', 'disarm_power_off', 'rename_account', 'orphan_worktrees',
  'remove_orphan_worktrees'
]);
const CORE_SEND: ReadonlySet<string> = new Set(['write_session', 'resize_session']);

const invokeAnywhere = <T>(cmd: string, args?: Record<string, unknown>): Promise<T> =>
  CORE_SEND.has(cmd)
    ? invoke<T>('core_send', { cmd, args: args ?? {} })
    : CORE_CALL.has(cmd)
      ? invoke<T>('core_call', { cmd, args: args ?? {} })
      : invoke<T>(cmd, args);

// The Rust side already logs why these failed, with more to say than this
// layer has; logging them here as well wrote every failure twice.
const LOGGED_BEHIND: ReadonlySet<string> = new Set(['account_limits', 'codex_limits']);

// In plain-browser dev (vite without tauri) all commands become no-ops so the
// UI stays workable.
async function call<T>(cmd: string, args?: Record<string, unknown>, fallback?: T): Promise<T> {
  if (!tauriAvailable) {
    // The README fixture answers the handful of commands whose emptiness would
    // show in a screenshot; without `?demo` it hands back the same fallback as
    // ever. Compiled out of a release build along with the module.
    if (import.meta.env.DEV) return (await import('../dev/demo')).answer(cmd, args ?? {}, fallback as T);
    return fallback as T;
  }
  // Timed because a sync Tauri command runs on the main thread: a slow one is
  // indistinguishable from the app hanging, and this names which one it was.
  const started = performance.now();
  try {
    return await invokeAnywhere<T>(cmd, args);
  } catch (e) {
    // A core that is not there fails every call until it is back, and the
    // banner already says so; one line per poll would bury the log.
    const noCore = String(e).includes('the core is not running');
    if (!LOGGED_BEHIND.has(cmd) && !noCore) logWarn('ipc', `${cmd} failed: ${String(e)}`);
    throw e;
  } finally {
    const ms = performance.now() - started;
    if (ms > (SLOW_BUDGET_MS[cmd] ?? SLOW_MS)) logWarn('ipc', `${cmd} took ${Math.round(ms)}ms`);
  }
}

// -------------------------------------------------------------- accounts --

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

export const getAccountsRoot = () =>
  call<AccountsRootInfo | null>('get_accounts_root', {}, null);
/** Empty string restores the default (Documents/luna-accounts). */
export const setAccountsRoot = (path: string) =>
  call<AccountsRootInfo>('set_accounts_root', { path });

export const createAccount = (provider: Provider, name: string) =>
  call<AccountInfo>('create_account', { provider, name });
export const deleteAccount = (provider: Provider, name: string) =>
  call<void>('delete_account', { provider, name });

// -------------------------------------------------------------- sessions --

export interface ClaudeSessionSpec {
  chatId: string;
  folder: string;
  accountPath: string;
  model: ModelLabel;
  effort: Effort;
  perm: PermMode;
  worktree: boolean;
  /** With `worktree`: the one to go into by name, made again if it is gone. */
  worktreeName?: string | null;
  resume?: string | null;
  /** Attach Luna's MCP tools (spawn/drive other sessions). */
  tools?: boolean;
  /** The chat whose agent spawned this one. */
  parent?: string | null;
  /** An opening prompt, for a chat an agent starts. */
  prompt?: string | null;
}

export const ensureClaudeSession = (spec: ClaudeSessionSpec) =>
  call<string>('ensure_claude_session', {
    id: spec.chatId,
    folder: spec.folder,
    accountPath: spec.accountPath,
    // Fallback covers chats persisted before the model labels changed.
    model: MODEL_CLI[spec.model] ?? spec.model.toLowerCase().split(' ')[0],
    effort: spec.effort,
    permissionMode: PERM_CLI[spec.perm],
    worktree: spec.worktree,
    worktreeName: spec.worktreeName ?? null,
    resume: spec.resume ?? null,
    tools: spec.tools ?? false,
    parent: spec.parent ?? null,
    prompt: spec.prompt ?? null
  }, '');

export interface CodexSessionSpec {
  chatId: string;
  /** Where Codex runs — the project, or a worktree it made earlier. */
  folder: string;
  /** A new session in a worktree of its own: Codex's `--worktree`. */
  worktree?: boolean;
  accountPath: string;
  model: string | null;
  effort: CodexEffort;
  approval: CodexApproval;
  sandbox: CodexSandbox;
  resume?: string | null;
  /** `codex login` instead of a chat. */
  login?: boolean;
  tools?: boolean;
  /** Run the project's hooks without their trust prompt (codex/hooks.rs). */
  hooksUnasked?: boolean;
  parent?: string | null;
  prompt?: string | null;
}

export const ensureCodexSession = (spec: CodexSessionSpec) =>
  call<string>('ensure_codex_session', {
    id: spec.chatId,
    folder: spec.folder,
    accountPath: spec.accountPath,
    model: spec.model,
    effort: spec.effort,
    approval: spec.approval,
    sandbox: spec.sandbox,
    worktree: spec.worktree ?? false,
    resume: spec.resume ?? null,
    login: spec.login ?? false,
    tools: spec.tools ?? false,
    hooksUnasked: spec.hooksUnasked ?? false,
    parent: spec.parent ?? null,
    prompt: spec.prompt ?? null
  }, '');

export const writeSession = (id: string, data: string) =>
  call<void>('write_session', { id, data });

export const resizeSession = (id: string, cols: number, rows: number) =>
  call<void>('resize_session', { id, cols, rows });

export const killSession = (id: string) => call<void>('kill_session', { id });

/** Escapes restoring the terminal modes the session's CLI switched. */
export const sessionModes = (id: string) => call<string>('session_modes', { id }, '');

export interface SessionMetaDto {
  name: string | null;
  status: string | null;
  cwd: string | null;
  sessionId: string | null;
  nameSource: string | null;
  /** The model the CLI reports running, where it records one (Codex). */
  model: string | null;
  context: number | null;
  contextTokens: number | null;
  contextWindow: number | null;
  /** The CLI's own title: a rename, else the one it generated. */
  title: string | null;
  /** `title` is the user's rename, not one the CLI made up. */
  titleRenamed: boolean;
  firstPrompt: string | null;
  /** A person left something typed and not sent in its composer. */
  draft: boolean;
  /** Messages Luna holds for it until that draft is sent or cleared. */
  held: number;
}

export interface CliTitleDto {
  title: string;
  renamed: boolean;
}

export const sessionMeta = (id: string, accountPath: string) =>
  call<SessionMetaDto | null>('session_meta', { id, accountPath }, null);

/** The CLI's title for a session that is not running, by its id. */
export const savedTitle = (provider: Provider, accountPath: string, sessionId: string) =>
  call<CliTitleDto | null>('saved_title', { provider, accountPath, sessionId }, null);

export interface OrphanSessionDto {
  id: string;
  provider: Provider;
  pid: number | null;
  cwd: string;
  accountPath: string;
  title: string | null;
  status: string | null;
}

/** Sessions this app is running that no chat in the sidebar claims. */
export const orphanSessions = (known: string[]) =>
  call<OrphanSessionDto[]>('orphan_sessions', { known }, []);

// ------------------------------------------------------------- worktrees --

/** Whether origin is fetched before a worktree is made (Settings). */
export const fetchBeforeWorktree = () => call<boolean>('fetch_before_worktree', {}, true);
export const setFetchBeforeWorktree = (on: boolean) => call<boolean>('set_fetch_before_worktree', { on }, on);

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



/**
 * Kills the session and cleans up after it in one call: the chat's attachments
 * always, its worktree and branch only when asked. Resolves the worktree
 * itself, so it works even when the UI never saw the path. Returns that
 * worktree whether or not it was removed.
 */
export const deleteSession = (
  chatId: string,
  folder: string,
  worktreePath: string | null,
  dropWorktree: boolean
) =>
  call<string | null>(
    'delete_session',
    { id: chatId, folder, worktreePath, dropWorktree },
    null
  );

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

// -------------------------------------------------------------- defaults --

/** Where a resolved default came from, for the line under the control;
 *  `stock` is Luna's own choice where no file said anything usable. */
export type ClaudeDefaultSource = 'account' | 'project' | 'project-local' | 'managed' | 'stock';
/** `codex`: what Codex itself would pick — the first model of its catalogue,
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
export const codexModels = (accountPath: string) =>
  call<CodexModelDto[]>('codex_models', { accountPath }, []);

// ----------------------------------------------------------------- trust --

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

// ----------------------------------------------------------------- media --

/** Copies a pasted/dropped file into the app's media store, returns its absolute path. */
export const saveMedia = (chatId: string, name: string, base64: string) =>
  call<string | null>('save_media', { chatId, name, data: base64 }, null);


// ------------------------------------------------------------------- cli --

export interface CliStatusDto {
  provider: Provider;
  phase: 'idle' | 'checking' | 'downloading' | 'error';
  /** Version Luna's own copy is at; null until the first download lands. */
  version: string | null;
  /** The binary sessions spawn — the bare name on PATH while there is no managed copy. */
  path: string;
  latest: string | null;
  got: number;
  total: number | null;
  error: string | null;
  checkedAtMs: number | null;
}

export const cliStatus = (provider: Provider) =>
  call<CliStatusDto | null>('cli_status', { provider }, null);
/** Check the release channel now and install whatever is newer. */
export const cliUpdateNow = (provider: Provider) => call<void>('cli_update_now', { provider });

export const pickFolder = async (): Promise<string | null> => {
  if (!tauriAvailable) return null;
  const { open } = await import('@tauri-apps/plugin-dialog');
  const res = await open({ directory: true, multiple: false });
  return typeof res === 'string' ? res : null;
};

// ----------------------------------------------------------------- power --

export type Turn = 'busy' | 'waiting' | 'idle';
export type PowerAction = 'shutdown' | 'hibernate' | 'sleep' | 'log';

export interface SessionActivityDto {
  id: string;
  turn: Turn;
  /** Exe names of the children keeping it busy, when any. */
  procs: string[];
  outputFresh: boolean;
  busy: boolean;
  /** Waiting only on what an API error left — a usage limit: counted as done. */
  stalled: string | null;
}

export interface ActivitySummaryDto {
  busy: number;
  waiting: number;
  stalled: number;
  idleSinceMs: number | null;
  sessions: SessionActivityDto[];
}

export interface PowerStateDto {
  keepAwake: boolean;
  /** The awake request is in force right now. */
  holding: boolean;
  armed: { action: PowerAction; quietS: number; armedAtMs: number } | null;
  countdownEndsAtMs: number | null;
  summary: ActivitySummaryDto;
}

export const IDLE_POWER: PowerStateDto = {
  keepAwake: true,
  holding: false,
  armed: null,
  countdownEndsAtMs: null,
  summary: { busy: 0, waiting: 0, stalled: 0, idleSinceMs: null, sessions: [] }
};

export const powerState = () => call<PowerStateDto>('power_state', {}, IDLE_POWER);
export const setKeepAwake = (on: boolean) => call<PowerStateDto>('set_keep_awake', { on }, IDLE_POWER);
export const armPowerOff = (action: PowerAction, quietS: number) =>
  call<PowerStateDto>('arm_power_off', { action, quietS }, IDLE_POWER);
export const disarmPowerOff = () => call<PowerStateDto>('disarm_power_off', {}, IDLE_POWER);

// ------------------------------------------------------------------ core --

export interface CoreDto {
  pid: number;
  version: string;
  build: string;
  fingerprint: string;
  started: number;
  sessions: number;
}

export interface CoreStateDto {
  state: 'starting' | 'up' | 'down' | 'mismatch';
  core: CoreDto | null;
  error: string | null;
}

export const coreState = () => call<CoreStateDto>('core_state', {}, { state: 'down', core: null, error: null });
export const coreRestart = () => call<void>('core_restart');

/** Why Luna is about to quit, for the log (lifecycle.rs); null takes it back. */
export const noteExit = (reason: string | null) => call<void>('note_exit', { reason });
/** Asks every session to quit as a delete would; resolves with how many. */
export const stopAllSessions = () => call<number>('stop_all_sessions', {}, 0);

// ---------------------------------------------------------------- agents --

/** What an agent asked for (agents.rs SpawnParams), as `agent://spawn` carries it. */
export interface AgentSpawnRequest {
  requestId: number;
  parentId: string;
  provider: Provider;
  account: string;
  model?: string | null;
  effort?: string | null;
  permissionMode?: string | null;
  approval?: string | null;
  sandbox?: string | null;
  folder: string;
  /** The caller's worktree, to run in without owning it; `folder` is its
   *  project. */
  inWorktree?: string | null;
  worktree?: boolean | null;
  prompt: string;
  name?: string | null;
  tools?: boolean | null;
  /** Codex: run the project's hooks unasked, as the folder starts a chat of
   *  the user's — the core decides, not the agent. */
  hooksUnasked: boolean;
}

/** What a session waits on a person for (agents.rs `Ask`). */
export interface AskDto {
  kind: 'question' | 'permission';
  blocking: boolean;
  questions?: Array<{ text: string; options?: string[]; multi?: boolean }>;
  tool?: string;
  detail?: string;
}

/** A helper that runs with no row (background.rs `Row`), listed under its parent. */
export interface BackgroundHelperDto {
  id: string;
  parent: string | null;
  name: string;
  provider: Provider;
  /** agents.rs `state_of`: working, waiting, question, idle… */
  state: string;
  ask?: AskDto | null;
  /** The account folder's name. */
  account: string;
  model: string | null;
  effort: string;
  worktree: boolean;
  startedMs: number;
  /** While a turn runs, when it started. */
  turnStartedMs: number | null;
}

/** A background helper handed over to a chat row (`agent://promote`). */
export interface PromotedDto {
  id: string;
  provider: Provider;
  accountPath: string;
  folder: string;
  worktreePath: string | null;
  sessionId: string | null;
  parent: string | null;
  name: string | null;
  /** The flags it ran with, in the CLI's own words. */
  settings: Record<string, unknown>;
  tools: boolean;
}

export const backgroundHelpers = () => call<BackgroundHelperDto[]>('background_helpers', {}, []);
/** Makes the helper a chat of its own; refused while a turn runs. */
export const backgroundPromote = (id: string) => call<void>('background_promote', { id });
/** Stops the turn it is on; the conversation stays. */
export const backgroundKill = (id: string) => call<void>('background_kill', { id });
export const backgroundDelete = (id: string) => call<void>('background_delete', { id });

/** Answers `agent://spawn`: the chat made for it, or why not. */
export const agentSpawned = (requestId: number, chatId: string | null, name: string | null, error: string | null) =>
  call<void>('agent_spawned', { requestId, chatId, name, error });

/** Every chat's name by id, for `luna_sessions` (agents.rs). */
export const chatNames = (names: Record<string, string>, groups: Record<string, number>) =>
  call<void>('chat_names', { names, groups });

/** One account's usage as `luna_list` gives it to agents. */
export interface AccountUsageDto {
  signedIn: boolean;
  plan: string;
  /** Percent used, by window, with the instant each resets. */
  limits: Array<{ label: string; usedPct: number; resetsAt: string | null }>;
  /** When the numbers were taken (ISO), null if none are known yet. */
  takenAt: string | null;
}

/** Every account's usage by `provider/name`, for `luna_list` (agents.rs). */
export const accountUsage = (usage: Record<string, AccountUsageDto>) => call<void>('account_usage', { usage });

/** `provider/name` keys of accounts agents may not use. */
export const agentBlockedAccounts = () => call<string[]>('agent_blocked_accounts', {}, []);
export const setAgentAccount = (provider: Provider, name: string, allowed: boolean) =>
  call<string[]>('set_agent_account', { provider, name, allowed }, []);

/** The page is alive: watchdog.rs rebuilds a window whose page stops saying so. */
export const uiHeartbeat = () => call<void>('ui_heartbeat');

// ------------------------------------------------------ account settings --

export interface RecommendationDto {
  /** Dotted path in the settings file, e.g. `tui` or `tui.show_tooltips`. */
  key: string;
  current: string | null;
  recommended: string;
  why: string;
  defaultOn: boolean;
  applied: boolean;
  /** The file it is kept in when that is not the account's settings file. */
  file: string | null;
}

export const recommendedSettings = (provider: Provider, accountPath: string) =>
  call<RecommendationDto[]>('recommended_settings', { provider, accountPath }, []);
/** Sets the `add` keys and takes the `remove` ones out of the file. */
export const applyRecommended = (provider: Provider, accountPath: string, add: string[], remove: string[]) =>
  call<void>('apply_recommended', { provider, accountPath, add, remove });

export interface SyncStateDto {
  claude: string | null;
  codex: string | null;
  onStart: boolean;
}
export interface SyncReportDto {
  synced: string[];
  failed: string[];
}

const NO_SYNC: SyncStateDto = { claude: null, codex: null, onStart: false };
export const settingsSyncState = () => call<SyncStateDto>('settings_sync_state', {}, NO_SYNC);
export const setSettingsParent = (provider: Provider, parent: string | null) =>
  call<SyncStateDto>('set_settings_parent', { provider, parent }, NO_SYNC);
export const setSettingsSyncOnStart = (on: boolean) => call<SyncStateDto>('set_settings_sync_on_start', { on }, NO_SYNC);
export const syncSettingsNow = (provider: Provider) => call<SyncReportDto>('sync_settings_now', { provider });

export const renameAccount = (provider: Provider, name: string, to: string) =>
  call<AccountInfo>('rename_account', { provider, name, to });

// --------------------------------------------------------------- library --

export type LibraryTarget = { kind: 'account'; provider: Provider; name: string } | { kind: 'folder'; path: string };
export type PlaceStatus = 'current' | 'behind' | 'missing' | 'changed' | 'gone' | 'taken' | 'skipped' | 'failed';

/** One copy the library wants somewhere, and where it stands. */
export interface PlaceDto {
  dest: string;
  target: LibraryTarget;
  /** Null for the instructions file. */
  skill: string | null;
  status: PlaceStatus;
  /** Changed there, and in the library too, since Luna wrote it. */
  both: boolean;
  problem: string | null;
}
/** Which accounts something goes into: `all`, a provider's every account
 *  (`claude`, `codex`) or one account (`provider/name`) per entry. */
export type Reach = string[];
export interface LibrarySkillDto {
  name: string;
  description: string;
  /** Why it is laid out nowhere, while it is not fit to be. */
  problem: string | null;
  accounts: Reach;
}
export interface LibraryOverviewDto {
  root: string;
  isDefault: boolean;
  /** False while the library's folder is not there. */
  found: boolean;
  instructions: Reach;
  hasInstructions: boolean;
  skills: LibrarySkillDto[];
  /** Every account, to pick a reach from. */
  accounts: { provider: Provider; name: string }[];
  folders: { path: string; skills: string[]; found: boolean }[];
  places: PlaceDto[];
  /** Accounts' instruction files Luna did not write, to start the library's from. */
  foundInstructions: { dest: string; target: LibraryTarget }[];
  /** Codex accounts whose AGENTS.override.md hides the AGENTS.md Luna writes. */
  overridden: string[];
  /** Copies no longer wanted, left in place because they had changed. */
  kept: string[];
}
export interface LibraryFileDto {
  path: string;
  text: string;
  exists: boolean;
}
/** One file of a copy as Luna wrote it last, as the library has it and as it is there. */
export interface FileDiffDto {
  path: string;
  base: string | null;
  library: string | null;
  there: string | null;
  binary: boolean;
}

export const libraryOverview = () => call<LibraryOverviewDto | null>('library_overview', {}, null);
/** Lays the library out — everything that needs no asking. */
export const librarySync = () => call<LibraryOverviewDto | null>('library_sync', {}, null);
export const librarySetSource = (path: string) => call<LibraryOverviewDto>('library_set_source', { path });
export const librarySetInstructions = (accounts: Reach) => call<LibraryOverviewDto>('library_set_instructions', { accounts });
export const librarySetAccounts = (skill: string, accounts: Reach) => call<LibraryOverviewDto>('library_set_accounts', { skill, accounts });
/** A folder's skills; null stops laying anything out into it. */
export const librarySetFolder = (path: string, skills: string[] | null) =>
  call<LibraryOverviewDto>('library_set_folder', { path, skills });
export const librarySkip = (dest: string, on: boolean) => call<LibraryOverviewDto>('library_skip', { dest, on });
/** `take`: the copy there becomes the library's; otherwise the library's goes back there. */
export const libraryResolve = (dest: string, take: boolean) => call<LibraryOverviewDto>('library_resolve', { dest, take });
export const libraryDiff = (dest: string) => call<FileDiffDto[]>('library_diff', { dest }, []);
/** The instructions (skill null) or a skill's SKILL.md. */
export const libraryRead = (skill: string | null) => call<LibraryFileDto>('library_read', { skill });
export const libraryWrite = (skill: string | null, text: string) => call<LibraryOverviewDto>('library_write', { skill, text });
export const libraryCheckSkill = (skill: string, text: string) =>
  call<string | null>('library_check_skill', { skill, text }, null);
export const libraryNewSkill = (name: string) => call<LibraryOverviewDto>('library_new_skill', { name });
export const libraryDeleteSkill = (name: string) => call<LibraryOverviewDto>('library_delete_skill', { name });
/** The library's folder, or a skill's, in the file manager. */
/** The tray icon's awake dot, with who holds the PC in its tooltip; null takes it off. */
export const trayAwake = (holders: string | null) => call<void>('tray_awake', { holders });
/** A browser a link can open in (browsers.rs): a name and a command line
 *  with `%1` where the link goes. */
export interface BrowserDto {
  id: string;
  name: string;
  command: string;
}

export interface BrowserSettingsDto {
  custom: BrowserDto[];
  /** Null: the system's handler. */
  default: string | null;
  /** By `provider/name`. */
  accounts: Record<string, string>;
  /** By project folder. */
  folders: Record<string, string>;
}

export interface BrowsersDto {
  /** The system handler's command line, where the system says. */
  system: string | null;
  found: BrowserDto[];
  settings: BrowserSettingsDto;
}

/** The id that stands for the system's handler. */
export const SYSTEM_BROWSER = 'system';

/** Where a link was clicked, for picking its browser: the chat's own choice,
 *  then its account's, its folder's, the default. */
export interface LinkContext {
  browser: string | null;
  account: string | null;
  folder: string | null;
}

export const browsers = () =>
  call<BrowsersDto>('browsers', {}, { system: null, found: [], settings: { custom: [], default: null, accounts: {}, folders: {} } });
export const setBrowsers = (settings: BrowserSettingsDto) => call<BrowsersDto>('set_browsers', { settings });
/** A link from a terminal, in the browser its context comes to (lib.rs `open_link`). */
export const openLink = (url: string, ctx: LinkContext) => call<void>('open_link', { url, ctx });
export const libraryOpen = (skill: string | null) => call<void>('library_open', { skill });

export interface ConfigFileDto {
  path: string;
  text: string;
  /** False when the account has no such file yet; `text` is a starting point. */
  exists: boolean;
}
export const readAccountConfig = (provider: Provider, accountPath: string) =>
  call<ConfigFileDto>('read_account_config', { provider, accountPath });
export const writeAccountConfig = (provider: Provider, accountPath: string, text: string) =>
  call<void>('write_account_config', { provider, accountPath, text });

export interface ConfigProblemDto {
  message: string;
  /** 1-based, when the parser says where. */
  line: number | null;
  column: number | null;
}
/** Where the text stops being a file the CLI can read; null when it reads. */
export const checkAccountConfig = (provider: Provider, text: string) =>
  call<ConfigProblemDto | null>('check_account_config', { provider, text }, null);
/** The text laid out the way the CLI writes it, comments and order kept. */
export const formatAccountConfig = (provider: Provider, text: string) =>
  call<string>('format_account_config', { provider, text });
