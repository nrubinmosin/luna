export type ChatStatus = 'working' | 'waiting' | 'resting';

/** Which window group a chat belongs to. Lives here rather than in the panes
 *  store so chats can carry it without the two stores importing each other. */
export type GroupId = 0 | 1 | 2 | 3;
export const GROUPS: GroupId[] = [0, 1, 2, 3];
export const GROUP_LABELS = ['I', 'II', 'III', 'IV'] as const;

export type Effort = 'low' | 'medium' | 'high' | 'xhigh' | 'max';

export type PermMode = 'Ask' | 'Edits' | 'Plan only' | 'Bypass';

export type ModelLabel = 'Opus' | 'Fable' | 'Haiku' | 'Sonnet';

export interface Chat {
  id: string;
  name: string;
  status: ChatStatus;
  model: ModelLabel;
  effort: Effort;
  perm: PermMode;
  context: number; // 0..1
  contextTokens?: number | null;
  contextWindow?: number | null;
  account: string;
  /** The group whose sidebar lists it. Chats are not shared between groups:
   *  each one is its own workspace, list and all. */
  group: GroupId;
  worktree: boolean;
  worktreePath?: string | null;
  sessionId?: string | null;
  nameCustom?: boolean;
  /** Folded away in the sidebar. Nothing else about the chat changes: the
   *  session, its worktree and its transcript are all left running. */
  archived?: boolean;
  /** A flag the user sets by hand, for their own bookkeeping. It means
   *  whatever they decide it means — nothing in the app reads it. */
  marked?: boolean;
  /** Key of a preset from CHAT_COLORS. The pane title bar wears it and the
   *  sidebar row shows a stripe of it; unset means stock Luna blue. */
  color?: string | null;
}

export interface Folder {
  id: string;
  path: string;
  open: boolean;
  chats: Chat[];
}

export interface LimitSet {
  h5: number;
  week: number;
  fable: number;
}

/** 'idle' before the first fetch, 'stale' while the CLI refreshes the token. */
export type AccountSync = 'loading' | 'ready' | 'stale' | 'throttled' | 'error';

export interface Account {
  name: string;
  path: string;
  plan: string;
  email: string | null;
  /** Whether the account can be used at all, independent of usage numbers. */
  signedIn: boolean;
  /** No usage figures known yet — render "—" instead of a confident 0%. */
  haveUsage: boolean;
  limits: LimitSet;
  resets: { h5: string; week: string; fable: string };
  /** Raw ISO instant the overall weekly limit resets, for showing the exact
   *  local date and time rather than a countdown. */
  weekResetAt: string | null;
  /** Age of the numbers, e.g. "just now" / "4m ago". */
  usageAge: string | null;
  /** When the numbers on screen were actually taken, so their age keeps
   *  ticking through rounds that brought nothing new. */
  fetchedAt: number | null;
  sync: AccountSync;
}

export type PaneIndex = 0 | 1 | 2 | 3;

export const MODELS: ModelLabel[] = ['Fable', 'Opus', 'Sonnet', 'Haiku'];
export const EFFORTS: Effort[] = ['low', 'medium', 'high', 'xhigh', 'max'];

export const MODEL_CLI: Record<ModelLabel, string> = {
  Opus: 'opus',
  Fable: 'fable',
  Haiku: 'haiku',
  Sonnet: 'sonnet'
};

export const PERM_CLI: Record<PermMode, string> = {
  Ask: 'default',
  Edits: 'acceptEdits',
  'Plan only': 'plan',
  Bypass: 'bypassPermissions'
};

export const PERM_HINTS: Record<PermMode, string> = {
  Ask: 'confirm every action',
  Edits: 'write files, ask for commands',
  'Plan only': 'no disk writes',
  Bypass: 'full access, no prompts'
};
