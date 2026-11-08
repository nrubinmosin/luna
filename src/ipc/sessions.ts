import { call } from './core';
import type { CodexApproval, CodexEffort, CodexSandbox, Effort, ModelLabel, PermMode, Provider } from '../shared/types';
import { MODEL_CLI, PERM_CLI } from '../shared/types';

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
  /** Without the edit tools. */
  readOnly?: boolean;
  /** The agent it runs as (`--agent`). */
  agent?: string | null;
}

export const ensureClaudeSession = (spec: ClaudeSessionSpec) =>
  call<string>(
    'ensure_claude_session',
    {
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
      readOnly: spec.readOnly ?? false,
      agent: spec.agent ?? null
    },
    ''
  );

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
}

export const ensureCodexSession = (spec: CodexSessionSpec) =>
  call<string>(
    'ensure_codex_session',
    {
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
      hooksUnasked: spec.hooksUnasked ?? false
    },
    ''
  );

export const writeSession = (id: string, data: string) => call<void>('write_session', { id, data });

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
  /** `waiting` comes from the marker its last message ends on (asking.rs),
   *  not from a prompt of its CLI's: the user may clear it by hand, by this key
   *  of the ending it was shown on. */
  marked: string | null;
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
export const orphanSessions = (known: string[]) => call<OrphanSessionDto[]>('orphan_sessions', { known }, []);

/** The text a last message ends on that makes its chat wait for the user. */
export interface WaitingMarker {
  on: boolean;
  text: string;
}
export const waitingMarker = () => call<WaitingMarker>('waiting_marker', {}, { on: true, text: 'AWAITING OPERATOR' });
export const setWaitingMarker = (on: boolean, text: string) =>
  call<WaitingMarker>('set_waiting_marker', { on, text }, { on, text });
/** A chat waiting by the marker is not waiting on you: cleared until a turn
 *  ends on the marker again. */
export const dismissWaiting = (id: string, key: string) => call<void>('dismiss_waiting', { id, key });
