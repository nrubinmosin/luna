import { call } from './core';
import type { Provider } from '../shared/types';

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
  startedMs: number;
  /** While a turn runs, when it started. */
  turnStartedMs: number | null;
  /** Between turns, what its turns left running ("tail.exe (pid 22104)"): what Stop ends then. */
  leftRunning?: string[];
  /** Being opened as a chat: no turn starts until the window says how that went (`backgroundPromoted`). */
  promoting?: boolean;
  /** When it last did anything: its last event, or a turn's start or end. */
  lastActivityMs: number | null;
  /** The first line of what it was spawned on. */
  task: string | null;
  /** The chat at the top of its line, `parent` managing it: "managed by <parent> · chat <top>". Null with no chat above it. */
  top?: string | null;
  /** The person's chats waiting on it now that are not above it: "watched by <chat>". */
  watchedBy?: string[];
  /** The person's last move of it to another chat: "moved from <from>, <when>". */
  moved?: HelperMoveDto;
  /** The person's chats refused a write into it, the latest last: "Move to <chat>?" (`backgroundMove`). */
  suggested?: MoveSuggestionDto[];
}

/** The person's last move of a helper to another chat (agents/reparent.rs `Moved`). */
export interface HelperMoveDto {
  /** The chat it was under; null for one that had no chat above it. */
  from: string | null;
  to: string;
  atMs: number;
  /** A turn of its own has been told of it. */
  told: boolean;
}

/** A chat of the person's refused a write into the helper, and when it last was (agents/reparent.rs `Suggested`). */
export interface MoveSuggestionDto {
  chat: string;
  atMs: number;
}

/** What the person's move did (agents/reparent.rs `HelperMoved`). */
export interface HelperMovedDto {
  id: string;
  from: string | null;
  to: string;
  /** Every session that moved: the helper and those under it. */
  sessions: string[];
  /** The autoDelete its chat's going had switched on is off again. */
  autoDeleteOff?: boolean;
  /** It was at work: the new chat is owed its answer. */
  owed?: boolean;
}

/** A chat a helper may move to, and why not where it may not (agents/reparent.rs `MoveTarget`). */
export interface MoveTargetDto {
  id: string;
  ok: boolean;
  why?: string;
}

/** Where a helper may move: `blocked` says why it cannot move at all (agents/reparent.rs `MoveTargets`). */
export interface MoveTargetsDto {
  blocked?: string;
  targets: MoveTargetDto[];
}

/** A background helper to be handed over to the person as a chat of their own (`backgroundPromote`):
 *  no parent, as it is nobody's helper once open. */
export interface PromotedDto {
  id: string;
  provider: Provider;
  accountPath: string;
  folder: string;
  worktreePath: string | null;
  sessionId: string | null;
  name: string | null;
  /** The flags it ran with, in the CLI's own words. */
  settings: Record<string, unknown>;
  tools: boolean;
}

/** What a helper has at stake in a delete (agents/teardown.rs `AtStake`). */
export interface AtStakeDto {
  id: string;
  /** What it is doing that a delete would cut off. */
  working?: string;
}

/** One message of a helper's conversation, as `luna_message_list` reads it. */
export interface HelperMessageDto {
  role: string;
  text: string;
}

/** A look at a helper's conversation (background/rows.rs `Transcript`). */
export interface HelperTranscriptDto {
  messages: HelperMessageDto[];
  /** Pass back as `since` for what comes after. */
  cursor: number;
  /** The file it was read from, passed back with `since`. */
  file: string | null;
  /** A turn is running. */
  running: boolean;
  /** Older messages a first look left out. */
  leftOut?: number;
  /** Part of the conversation was passed over unread: too far back, or a line too long. */
  passedOver?: boolean;
}

export const backgroundHelpers = () => call<BackgroundHelperDto[]>('background_helpers', {}, []);
/** Starts handing the helper over to the person as a chat of their own, refused while a turn runs:
 *  what the row needs. The helper stays, with no turn starting and out of its parent's reach, until
 *  `backgroundPromoted` says how the row came out. */
export const backgroundPromote = (id: string) => call<PromotedDto>('background_promote', { id });
/** The row for a helper being opened is made (`error` null), and it is the person's for good; or it
 *  could not be, and the helper goes on as its parent's. Whether it was handed over: false for a row
 *  made for an opening the core has since given up, the helper its parent's again. */
export const backgroundPromoted = (id: string, error: string | null) =>
  call<boolean>('background_promoted', { id, error });
/** Stops the turn it is on, or between turns what its turns left running; the conversation stays. */
export const backgroundKill = (id: string) => call<void>('background_kill', { id });
/** Deletes it; a worktree it ran in stays, as Luna removes none. A helper at work is refused unless
 *  `stopWorking`; with `ofChat`, one that hangs from another chat by now is refused too. */
export const backgroundDelete = (
  id: string,
  { stopWorking = false, ofChat = null }: { stopWorking?: boolean; ofChat?: string | null } = {}
) => call<void>('background_delete', { id, stopWorking, ofChat });
/** Moves the helper, with the helpers under it, to the person's chat `to`; rejects with why not,
 *  nothing changed. */
export const backgroundMove = (id: string, to: string) => call<HelperMovedDto>('background_move', { id, to });
/** The person's chats the helper may move to, each with why not where it may not. */
export const backgroundMoveTargets = (id: string) =>
  call<MoveTargetsDto>('background_move_targets', { id }, { targets: [] });
/** What each helper has at stake in a delete, looked at now. */
export const backgroundAtStake = (ids: string[]) => call<AtStakeDto[]>('background_at_stake', { ids }, []);
/** A helper's conversation from `since` on, read-only (`file`: the one `since` was a cursor into). */
export const backgroundTranscript = (id: string, since: number | null, file: string | null) =>
  call<HelperTranscriptDto>('background_transcript', { id, since, file });

/** Every chat's name by id, for `luna_session_list` (agents/). */
export const chatNames = (names: Record<string, string>, groups: Record<string, number>) =>
  call<void>('chat_names', { names, groups });

/** One account's usage as `luna_account_list` gives it to agents. */
export interface AccountUsageDto {
  signedIn: boolean;
  plan: string;
  /** Percent used, by window, with the instant each resets. */
  limits: Array<{ label: string; usedPct: number; resetsAt: string | null }>;
  /** When the numbers were taken (ISO), null if none are known yet. */
  takenAt: string | null;
}

/** Every account's usage by `provider/name`, for `luna_account_list` (agents/). */
export const accountUsage = (usage: Record<string, AccountUsageDto>) => call<void>('account_usage', { usage });

/** `provider/name` keys of accounts agents may not use. */
export const agentBlockedAccounts = () => call<string[]>('agent_blocked_accounts', {}, []);
export const setAgentAccount = (provider: Provider, name: string, allowed: boolean) =>
  call<string[]>('set_agent_account', { provider, name, allowed }, []);

/** The places a line of sessions has, the range they may be set in, and the
 *  share of a helper's context past which its notices say so (0 for off)
 *  (agents/spawn `HelperSettings`). */
export interface HelperSettingsDto {
  maxHelpers: number;
  min: number;
  max: number;
  helperContextPct: number;
}

const HELPER_DEFAULTS: HelperSettingsDto = {
  maxHelpers: 8,
  min: 1,
  max: 32,
  helperContextPct: 50
};
export const helperSettings = () => call<HelperSettingsDto>('helper_settings', {}, HELPER_DEFAULTS);
export const setMaxHelpers = (n: number) =>
  call<HelperSettingsDto>('set_max_helpers', { n }, { ...HELPER_DEFAULTS, maxHelpers: n });

export const setHelperContextPct = (pct: number) =>
  call<HelperSettingsDto>('set_helper_context_pct', { pct }, { ...HELPER_DEFAULTS, helperContextPct: pct });

/** The page is alive: watchdog.rs rebuilds a window whose page stops saying so. */
export const uiHeartbeat = () => call<void>('ui_heartbeat');
