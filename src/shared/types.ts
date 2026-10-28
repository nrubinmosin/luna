export type ChatStatus = 'working' | 'waiting' | 'resting';

/** Which window group a chat belongs to: an index into the panes store's
 *  groups. Lives here rather than in the panes store so chats can carry it
 *  without the two stores importing each other. */
export type GroupId = number;
/** As many groups as the title bar has room for, and as Roman numerals stay
 *  short enough to read at a glance. */
export const MAX_GROUPS = 10;
const ROMAN = ['I', 'II', 'III', 'IV', 'V', 'VI', 'VII', 'VIII', 'IX', 'X'] as const;
export const groupLabel = (g: GroupId): string => ROMAN[g] ?? String(g + 1);

/** Where a chat's name came from — see `Chat.nameSource`. */
export type NameSource = 'user' | 'rename' | 'cli' | 'prompt';

// ------------------------------------------------------------- providers --

/** The two CLIs Luna drives. Each keeps its own vocabulary end to end: a
 *  Codex chat never wears a Claude permission mode and vice versa. */
export type Provider = 'claude' | 'codex';
export const PROVIDERS: Provider[] = ['claude', 'codex'];
export const PROVIDER_LABEL: Record<Provider, string> = { claude: 'Claude Code', codex: 'Codex' };
/** Whose subscription the account is. */
export const PROVIDER_VENDOR: Record<Provider, string> = { claude: 'Anthropic', codex: 'OpenAI' };

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
/** `auto-review` is Codex's `--approve-for-me`: what it would ask goes to a
 *  reviewer model instead, always in `workspace-write`. */
export type CodexApproval = 'on-request' | 'never' | 'auto-review';
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
  /** Where the name came from, which decides what may still replace it:
   *  - `user`: given in Luna. Final — nothing replaces it.
   *  - `rename`: a `/rename` in the CLI. Only another one replaces it.
   *  - `cli`: the title the CLI made up. Taken once; only a rename replaces it.
   *  - `prompt`: the opening prompt, until the CLI's title lands.
   *  - unset: the placeholder ("chat 12") a new chat starts with. */
  nameSource?: NameSource;
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
  /** A person launched a chat here or browsed to it, which is what puts it
   *  in the new-chat dialog. A folder only an agent ran a session in holds
   *  that session's row and nothing more. */
  picked?: boolean;
  /** How the new-chat checkboxes start here, when kept for this folder from
   *  the dialog; none means the defaults in Settings. Lives on the folder so
   *  that forgetting the folder forgets this too. */
  start?: FolderStart;
}

export interface FolderStart {
  worktree: boolean;
  tools: boolean;
}

// -------------------------------------------------------------- accounts --

/** One usage window, the same shape for both providers. */
export interface LimitWindow {
  id: string;
  /** "5 hours", "week", a model ("Fable"), or the limit's own name. */
  label: string;
  /** The status bar's key for it: "5H", "W", "F" (Fable). */
  short: string;
  used: number; // 0..1
  /** The instant it resets; countdowns are worked out from it as they show. */
  resetAt: string | null;
}

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
  /** Every usage window the provider reports, the shortest first. */
  limits: LimitWindow[];
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
export const CODEX_APPROVALS: CodexApproval[] = ['on-request', 'never', 'auto-review'];
export const CODEX_SANDBOXES: CodexSandbox[] = ['read-only', 'workspace-write', 'danger-full-access'];
/** The one sandbox `auto-review` runs in. */
export const AUTO_REVIEW_SANDBOX: CodexSandbox = 'workspace-write';

export const CODEX_APPROVAL_HINTS: Record<CodexApproval, string> = {
  'on-request': 'Codex asks you before anything the sandbox would block',
  never: 'never asks — blocked actions just fail',
  'auto-review': 'a reviewer model answers what Codex would ask you, and the turn goes on'
};

export const CODEX_SANDBOX_HINTS: Record<CodexSandbox, string> = {
  'read-only': 'commands can only read the disk',
  'workspace-write': 'writes inside the folder and temp, nowhere else',
  'danger-full-access': 'no sandbox at all'
};

/** What an approval and a sandbox come to together, as one line. */
export function codexModeHint(approval: CodexApproval, sandbox: CodexSandbox): string {
  if (approval === 'auto-review') return `Runs with --approve-for-me: ${CODEX_APPROVAL_HINTS[approval]}; ${CODEX_SANDBOX_HINTS[AUTO_REVIEW_SANDBOX]}.`;
  if (sandbox === 'danger-full-access') {
    return approval === 'never'
      ? 'Runs with --dangerously-bypass-approvals-and-sandbox.'
      : 'No sandbox; Codex still asks you before commands it finds dangerous.';
  }
  return `${CODEX_APPROVAL_HINTS[approval]}; ${CODEX_SANDBOX_HINTS[sandbox]}.`;
}

export const codexEffortFromSetting = (raw: string): CodexEffort | null => {
  const v = raw.trim().toLowerCase();
  return /^[a-z]+$/.test(v) ? v : null;
};
export const codexApprovalFromSetting = (raw: string): CodexApproval | null =>
  CODEX_APPROVALS.find(e => e === raw.toLowerCase()) ?? null;
export const codexSandboxFromSetting = (raw: string): CodexSandbox | null =>
  CODEX_SANDBOXES.find(e => e === raw.toLowerCase()) ?? null;
