import { call } from './core';
import type { ClaudeSettings, CodexSettings } from '../shared/types';

/** The preview of a move to another account (agents/handoff/moving.rs
 *  `MovePreview`): the line Luna will type, fixed, and the document, which the
 *  person may edit before Send. */
export interface MovePreviewDto {
  moveId: string;
  newId: string;
  prompt: string;
  text: string;
  sha256: string;
  /** Secret-like strings taken out of it. */
  redacted: number;
  /** Folders of accounts and of Luna's cut out of it. */
  pathsCut: number;
  /** Long random strings left in, for the person to look at. */
  highlights: string[];
  /** Characters no screen shows, taken out of it. */
  invisible: number;
  stopOld: boolean;
  worktree: boolean;
  warnings: string[];
  maxBytes: number;
}

export type MovePhase =
  | 'asking'
  | 'asked'
  | 'shown'
  | 'confirmed'
  | 'oldStopped'
  | 'rowCreated'
  | 'prompting'
  | 'done'
  | 'failed'
  | 'canceled';

/** The settings the new chat starts with, as the window sends them with Send:
 *  the core binds them to the text and the account, and holds the new chat's
 *  start to them. */
export interface MoveLaunch {
  provider: 'claude' | 'codex';
  account: string;
  accountPath: string;
  /** The project folder the new row is filed under and starts in. */
  folder: string;
  stopOld: boolean;
  name: string;
  color: string | null;
  group: number;
  tools: boolean;
  guard: boolean;
  autoHandoff: boolean;
  handoffAt?: number;
  /** A Claude Code worktree the old chat ran in, gone into by name. */
  worktreeName?: string | null;
  worktreePath?: string | null;
  model?: string | null;
  effort?: string;
  perm?: ClaudeSettings['perm'];
  /** `perm` as Claude Code's flag spells it: what the core holds the start to. */
  permissionMode?: string;
  readOnly?: boolean;
  agent?: string | null;
  approval?: CodexSettings['approval'];
  sandbox?: CodexSettings['sandbox'];
  hooksUnasked?: boolean;
  /** Codex never makes a worktree for a moved chat. */
  worktree?: false;
}

/** A usage limit that still holds a chat (activity/limit.rs `LimitFact`):
 *  its kind, since when, and the window that is full with its reset, by the
 *  reading it was judged on and how old that was. */
export interface LimitFact {
  kind: string;
  since: number;
  window?: string | null;
  /** 0..1 */
  used?: number | null;
  resetsAt?: string | null;
  readingAt?: number | null;
  readingAgeMs?: number | null;
}

export interface MoveStatusDto {
  moveId: string;
  chatId: string;
  newId: string;
  phase: MovePhase;
  at: number;
  why?: string | null;
  stopOld: boolean;
  worktree: boolean;
  toAccount: string;
  source: 'luna' | 'chat';
  launch?: MoveLaunch | null;
  /** An opening that waits: on what, and since when. */
  waiting?: string | null;
  waitingSince?: number | null;
  /** Its screen shows an empty composer: `Type it now` may be offered. */
  composerSeen?: boolean;
  /** The chat was asked for its document and the move failed before it came
   *  (`why` says how): Luna may build it from records instead. */
  askFailed?: boolean;
  /** After such a failure, asking the chat again may go through. */
  askAgain?: boolean;
  /** When Luna typed its ask into the chat. */
  askedAt?: number | null;
  /** The usage limit that still holds the old chat, while its document is
   *  still to be had. */
  limit?: LimitFact | null;
}

/** What the window looks at before a move begins: a usage limit still in
 *  force, which has it offer Luna's document from records first, and what
 *  keeps the chat from being asked for its own. */
export interface MoveFactsDto {
  limit?: LimitFact | null;
  askBlocked?: string | null;
}

/** Builds the document and keeps the move as shown; nothing goes anywhere. */
export const movePrepare = (
  chatId: string,
  newId: string,
  toAccountPath: string,
  sessionId: string | null,
  note: string
) => call<MovePreviewDto>('move_prepare', { chatId, newId, toAccountPath, sessionId, note });

/** Send: the text as confirmed, its SHA-256 as shown, and the launch. */
export const moveConfirm = (chatId: string, moveId: string, text: string, sha256: string, launch: MoveLaunch) =>
  call<MoveStatusDto>('move_confirm', { chatId, moveId, text, sha256, launch });

/** A step of the window's: the old chat stopped by the core once it has looked
 *  at what its worktree holds (`stopOld`), or the new row made. */
export const moveStep = (chatId: string, moveId: string, step: 'stopOld' | 'oldStopped' | 'rowCreated' | 'typeNow') =>
  call<MoveStatusDto>('move_step', { chatId, moveId, step });

export const moveCancel = (chatId: string, moveId: string) => call<MoveStatusDto>('move_cancel', { chatId, moveId });

/** Asks a chat with Luna's tools to write its own document; it is collected
 *  through its handoff tool, nothing reset. */
export const moveAsk = (chatId: string, newId: string, toAccountPath: string, note: string) =>
  call<MoveStatusDto>('move_ask', { chatId, newId, toAccountPath, note });

/** The preview of a document the chat wrote, once collected. */
export const movePreview = (chatId: string, moveId: string) => call<MovePreviewDto>('move_preview', { chatId, moveId });

/** Closes the chat's ask for a move being asked, or whose ask failed, and
 *  builds the document from records into the same preview, the destination
 *  and the note kept. */
export const moveBuildFromRecords = (chatId: string, moveId: string, sessionId: string | null) =>
  call<MovePreviewDto>('move_build_from_records', { chatId, moveId, sessionId });

/** A chat's usage limit still in force, and what keeps it from being asked. */
export const moveFacts = (chatId: string) => call<MoveFactsDto>('move_facts', { chatId });

/** Every move under way, and those ended lately. */
export const moveList = () => call<MoveStatusDto[]>('move_list', {}, []);

/** SHA-256 of `text` as UTF-8, hex: what Send is bound to. */
export async function sha256Hex(text: string): Promise<string> {
  const digest = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(text));
  return [...new Uint8Array(digest)].map(b => b.toString(16).padStart(2, '0')).join('');
}
