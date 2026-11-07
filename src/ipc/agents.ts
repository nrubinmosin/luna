import { call } from './core';
import type { Provider } from '../shared/types';

/** What an agent asked for (agents/spawn/mod.rs SpawnParams), as `agent://spawn` carries it. */
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
  readOnly?: boolean | null;
  agent?: string | null;
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

/** What a session waits on a person for (agents/ask.rs `Ask`). */
export interface AskDto {
  kind: 'question' | 'permission';
  blocking: boolean;
  questions?: Array<{ text: string; options?: string[]; multi?: boolean }>;
  tool?: string;
  detail?: string;
}

/** A helper that runs with no row (background/rows.rs `Row`), listed under its parent. */
export interface BackgroundHelperDto {
  id: string;
  parent: string | null;
  name: string;
  provider: Provider;
  /** agents/ask.rs `state_of`: working, waiting, question, idle… */
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
  /** Between turns, what its turns left running ("tail.exe (pid 22104)"): what Stop ends then. */
  leftRunning?: string[];
  /** Being opened as a chat: no turn starts until the window says how that went (`backgroundPromoted`). */
  promoting?: boolean;
}

/** A background helper to be handed over to a chat row (`backgroundPromote`). */
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
/** Starts making the helper a chat of its own, refused while a turn runs: what the row needs. The
 *  helper stays, with no turn starting, until `backgroundPromoted` says how the row came out. */
export const backgroundPromote = (id: string) => call<PromotedDto>('background_promote', { id });
/** The row for a helper being opened is made (`error` null), and the core lets the helper go; or it
 *  could not be, and the helper goes on as it was. */
export const backgroundPromoted = (id: string, error: string | null) =>
  call<void>('background_promoted', { id, error });
/** Stops the turn it is on, or between turns what its turns left running; the conversation stays. */
export const backgroundKill = (id: string) => call<void>('background_kill', { id });
/** Deletes it, the worktree it made too unless `dropWorktree` is false. */
export const backgroundDelete = (id: string, dropWorktree = true) =>
  call<void>('background_delete', { id, dropWorktree });

/** Answers `agent://spawn`: the chat made for it, or why not. */
export const agentSpawned = (requestId: number, chatId: string | null, name: string | null, error: string | null) =>
  call<void>('agent_spawned', { requestId, chatId, name, error });

/** The chats the core wants started to go on by themselves (agents/resume.rs `resume_chats`). */
export const agentResumes = () => call<string[]>('agent_resumes');
/** What the window has been asked to start and nothing has come of yet: an event
 *  reaches only a page that is listening. */
export const agentSpawns = () => call<AgentSpawnRequest[]>('agent_spawns', {}, []);

/** Every chat's name by id, for `luna_sessions` (agents/). */
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

/** Every account's usage by `provider/name`, for `luna_list` (agents/). */
export const accountUsage = (usage: Record<string, AccountUsageDto>) => call<void>('account_usage', { usage });

/** `provider/name` keys of accounts agents may not use. */
export const agentBlockedAccounts = () => call<string[]>('agent_blocked_accounts', {}, []);
export const setAgentAccount = (provider: Provider, name: string, allowed: boolean) =>
  call<string[]>('set_agent_account', { provider, name, allowed }, []);

/** Whether helpers may run with more privilege than the session spawning them
 *  (Settings; agents/privilege.rs `cap`). */
export const helpersAboveCaller = () => call<boolean>('helpers_above_caller', {}, false);
export const setHelpersAboveCaller = (on: boolean) => call<boolean>('set_helpers_above_caller', { on }, on);

/** The page is alive: watchdog.rs rebuilds a window whose page stops saying so. */
export const uiHeartbeat = () => call<void>('ui_heartbeat');
