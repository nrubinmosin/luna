export type ChatStatus = 'working' | 'waiting' | 'resting';

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
  worktree: boolean;
  worktreePath?: string | null;
  sessionId?: string | null;
  nameCustom?: boolean;
  /** Folded away in the sidebar. Nothing else about the chat changes: the
   *  session, its worktree and its transcript are all left running. */
  archived?: boolean;
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
  /** Age of the numbers, e.g. "just now" / "4m ago". */
  usageAge: string | null;
  sync: AccountSync;
}

export type PaneIndex = 0 | 1 | 2 | 3;

export const MODELS: ModelLabel[] = ['Opus', 'Fable', 'Haiku', 'Sonnet'];
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
