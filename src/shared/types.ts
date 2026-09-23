export type ChatStatus = 'working' | 'waiting' | 'resting';

/** Which window group a chat belongs to. Lives here rather than in the panes
 *  store so chats can carry it without the two stores importing each other. */
export type GroupId = 0 | 1 | 2 | 3;
export const GROUPS: GroupId[] = [0, 1, 2, 3];
export const GROUP_LABELS = ['I', 'II', 'III', 'IV'] as const;

// ------------------------------------------------------------- providers --

/** The two CLIs Luna drives. Each keeps its own vocabulary end to end: a
 *  Codex chat never wears a Claude permission mode and vice versa. */
export type Provider = 'claude' | 'codex';
export const PROVIDERS: Provider[] = ['claude', 'codex'];
export const PROVIDER_LABEL: Record<Provider, string> = { claude: 'Claude Code', codex: 'Codex' };
/** Whose subscription the account is, and the sub-folder it lives in. */
export const PROVIDER_VENDOR: Record<Provider, string> = { claude: 'Anthropic', codex: 'OpenAI' };
export const PROVIDER_DIR: Record<Provider, string> = { claude: 'anthropic', codex: 'openai' };

// ---------------------------------------------------------- Claude Code --

export type Effort = 'low' | 'medium' | 'high' | 'xhigh' | 'max' | 'ultracode';

export type PermMode = 'Ask' | 'Edits' | 'Plan only' | 'Bypass';

export type ModelLabel = 'Opus' | 'Fable' | 'Haiku' | 'Sonnet';

/** The three things a Claude Code settings file can have an opinion about. */
export interface ClaudeSettings {
  model: ModelLabel;
  effort: Effort;
  perm: PermMode;
}

// ---------------------------------------------------------------- Codex --

/** A reasoning level as Codex names it. The set is per model and comes off
 *  the CLI's own model list (`low … ultra` today), so it is not closed here. */
export type CodexEffort = string;
export type CodexApproval = 'on-request' | 'never';
export type CodexSandbox = 'read-only' | 'workspace-write' | 'danger-full-access';

/** What Codex's `config.toml` can say and Luna passes as flags. The model is
 *  a free string — Codex's list moves too fast to enumerate — and null means
 *  whatever Codex itself defaults to. */
export interface CodexSettings {
  model: string | null;
  effort: CodexEffort;
  approval: CodexApproval;
  sandbox: CodexSandbox;
}

// ----------------------------------------------------------------- chats --

interface ChatBase {
  id: string;
  name: string;
  status: ChatStatus;
  context: number; // 0..1
  contextTokens?: number | null;
  contextWindow?: number | null;
  /** Account name; the provider says which sub-folder it is under. */
  account: string;
  /** The group whose sidebar lists it. Chats are not shared between groups:
   *  each one is its own workspace, list and all. */
  group: GroupId;
  worktree: boolean;
  worktreePath?: string | null;
  sessionId?: string | null;
  nameCustom?: boolean;
  /** Key of a preset from CHAT_COLORS. The pane title bar wears it and the
   *  sidebar row shows a stripe of it; unset means stock Luna blue. */
  color?: string | null;
  /** Luna's MCP tools attached: this session can spawn and drive others. */
  tools?: boolean;
  /** The chat whose agent spawned this one. A parent that no longer exists
   *  leaves the child an orphan, listed at the top level with a mark. */
  parentId?: string | null;
  /** Whether the children under this row are unfolded; folded by default. */
  childrenOpen?: boolean;
}

export interface ClaudeChat extends ChatBase, ClaudeSettings {
  provider: 'claude';
}

export interface CodexChat extends ChatBase, CodexSettings {
  provider: 'codex';
  /** The model Codex actually reports running, off its rollout — what the
   *  title bar shows when the chat setting is "Codex default". */
  modelSeen?: string | null;
}

export type Chat = ClaudeChat | CodexChat;

export interface Folder {
  id: string;
  path: string;
  open: boolean;
  chats: Chat[];
}

// -------------------------------------------------------------- accounts --

export interface ClaudeLimits {
  kind: 'claude';
  h5: number;
  week: number;
  fable: number;
  resets: { h5: string; week: string; fable: string };
  /** Raw ISO instant the overall weekly limit resets, for showing the exact
   *  local date and time rather than a countdown. */
  weekResetAt: string | null;
}

export interface CodexLimitWindow {
  id: string;
  /** "5 hours", "week", or the limit's own name. */
  label: string;
  used: number; // 0..1
  /** Countdown, e.g. "3h 40m". */
  reset: string;
  resetAt: string | null;
}

export interface CodexLimits {
  kind: 'codex';
  windows: CodexLimitWindow[];
}

export const EMPTY_CLAUDE_LIMITS: ClaudeLimits = {
  kind: 'claude', h5: 0, week: 0, fable: 0, resets: { h5: '—', week: '—', fable: '—' }, weekResetAt: null
};
export const EMPTY_CODEX_LIMITS: CodexLimits = { kind: 'codex', windows: [] };

/** 'loading' before the first fetch, 'stale' while an expired token has not
 *  been renewed yet (by Luna for Claude Code, by the CLI itself for Codex). */
export type AccountSync = 'loading' | 'ready' | 'stale' | 'throttled' | 'error';

export interface Account {
  provider: Provider;
  name: string;
  path: string;
  plan: string;
  email: string | null;
  /** Whether the account can be used at all, independent of usage numbers. */
  signedIn: boolean;
  /** No usage figures known yet — render "—" instead of a confident 0%. */
  haveUsage: boolean;
  limits: ClaudeLimits | CodexLimits;
  /** Age of the numbers, e.g. "just now" / "4m ago". */
  usageAge: string | null;
  /** When the numbers on screen were actually taken, so their age keeps
   *  ticking through rounds that brought nothing new. */
  fetchedAt: number | null;
  sync: AccountSync;
  /** Why renewing an expired token failed, when Luna tried. */
  refreshError: string | null;
}

/** `codex/work` — one string that names an account across both providers. */
export const accountKey = (provider: Provider, name: string) => `${provider}/${name}`;

export type PaneIndex = 0 | 1 | 2 | 3;

// ------------------------------------------------ Claude Code's own settings

export const MODELS: ModelLabel[] = ['Fable', 'Opus', 'Sonnet', 'Haiku'];
export const EFFORTS: Effort[] = ['low', 'medium', 'high', 'xhigh', 'max', 'ultracode'];

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

/** `permissions.defaultMode` as the CLI spells it, back onto Luna's labels. */
const PERM_OF_CLI: Record<string, PermMode> = {
  default: 'Ask',
  acceptEdits: 'Edits',
  plan: 'Plan only',
  bypassPermissions: 'Bypass'
};

/**
 * `model` from a settings file. It can be an alias (`opus`), a compound one
 * (`opusplan`) or a full id (`claude-sonnet-5`); all three name their model
 * plainly enough to match on, and anything else means Luna keeps its own.
 */
export const modelFromSetting = (raw: string): ModelLabel | null => {
  const v = raw.toLowerCase();
  return MODELS.find(m => v.includes(m.toLowerCase())) ?? null;
};

export const effortFromSetting = (raw: string): Effort | null =>
  EFFORTS.find(e => e === raw.toLowerCase()) ?? null;

export const permFromSetting = (raw: string): PermMode | null => PERM_OF_CLI[raw] ?? null;

// ------------------------------------------------------ Codex's own settings

/** The levels shown when the account has no model list cached yet. */
export const CODEX_EFFORTS: CodexEffort[] = ['low', 'medium', 'high', 'xhigh', 'max', 'ultra'];
export const CODEX_APPROVALS: CodexApproval[] = ['on-request', 'never'];
export const CODEX_SANDBOXES: CodexSandbox[] = ['read-only', 'workspace-write', 'danger-full-access'];

export const CODEX_APPROVAL_HINTS: Record<CodexApproval, string> = {
  'on-request': 'Codex asks before anything the sandbox would block',
  never: 'never asks — blocked actions just fail'
};

export const CODEX_SANDBOX_HINTS: Record<CodexSandbox, string> = {
  'read-only': 'commands can only read the disk',
  'workspace-write': 'writes inside the folder; needs the Windows sandbox set up, else acts as read-only',
  'danger-full-access': 'no sandbox at all'
};

/** Pairs Codex itself names: the `--dangerously-bypass-approvals-and-sandbox`
 *  flag, and the two everyday combinations. Buttons in the dialog, nothing
 *  a chat stores. */
export const CODEX_PRESETS: Array<{ label: string; approval: CodexApproval; sandbox: CodexSandbox; hint: string }> = [
  { label: 'Bypass', approval: 'never', sandbox: 'danger-full-access', hint: 'full access, no prompts' },
  { label: 'Auto', approval: 'on-request', sandbox: 'workspace-write', hint: 'edits in place, asks to go outside' },
  { label: 'Read only', approval: 'on-request', sandbox: 'read-only', hint: 'asks before any write' }
];

export const codexEffortFromSetting = (raw: string): CodexEffort | null => {
  const v = raw.trim().toLowerCase();
  return /^[a-z]+$/.test(v) ? v : null;
};
export const codexApprovalFromSetting = (raw: string): CodexApproval | null =>
  CODEX_APPROVALS.find(e => e === raw.toLowerCase()) ?? null;
export const codexSandboxFromSetting = (raw: string): CodexSandbox | null =>
  CODEX_SANDBOXES.find(e => e === raw.toLowerCase()) ?? null;
