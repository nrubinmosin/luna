/**
 * The fixture behind the README screenshots.
 *
 * Shooting the real app means shooting whatever is open in it — someone's
 * folders, chat titles, account emails and half a client's codebase. This
 * stands the same UI up on invented data instead: `pnpm dev` with `?demo` in
 * the URL, no Tauri and no sessions, so the shots can be retaken from a clean
 * slate whenever the UI moves on. See `screenshots.ps1`.
 *
 * Dev-only by construction: every entry point is behind `import.meta.env.DEV`,
 * so none of this is in a release build.
 */
import type { Terminal as XTerm } from '@xterm/xterm';
import type { Account, Chat, ClaudeChat, CodexChat, Folder, Provider } from '../shared/types';
import type {
  AccountsRootInfo, BackgroundHelperDto, WorktreeInfoDto, ClaudeDefaultsDto, CliStatusDto, CodexDefaultsDto, CodexModelDto, PowerStateDto,
  FileDiffDto, LibraryOverviewDto, PlaceDto
} from '../ipc/commands';
import { useLibrary } from '../features/library/library.store';
import { useBackground } from '../features/agents/background.store';
import { useAccounts } from '../features/accounts/accounts.store';
import { useAccountsWindow } from '../features/accounts/AccountsPanel';
import { useChats } from '../features/chats/chats.store';
import { useDrafts } from '../features/chats/drafts.store';
import { useWorktrees } from '../features/chats/worktrees.store';
import { useNewChat } from '../features/new-chat/newchat.store';
import { usePanes, type Group, type Layout, type Slots } from '../features/panes/panes.store';
import { useUpdates } from '../features/updates/updates.store';
import { useShell } from '../app/shell.store';

/** `?demo`, `?demo=dark`, `?demo=peek`, `?demo=newchat`, `?demo=codex` (the same on a Codex
 *  account), `?demo=settings`, `?demo=accounts`, `?demo=recommended`, `?demo=editor`, `?demo=worktrees`,
 *  `?demo=library`. */
export type Scene = 'main' | 'dark' | 'peek' | 'newchat' | 'codex' | 'settings' | 'accounts' | 'recommended' | 'editor' | 'worktrees' | 'library';
const SCENES: Scene[] = ['dark', 'peek', 'newchat', 'codex', 'settings', 'accounts', 'recommended', 'editor', 'worktrees', 'library'];

const hoursFromNow = (h: number) => new Date(Date.now() + h * 3600_000).toISOString();

const chat = (c: Partial<ClaudeChat> & Pick<Chat, 'id' | 'name'>): ClaudeChat => ({
  provider: 'claude',
  status: 'resting',
  model: 'Opus',
  effort: 'high',
  perm: 'Bypass',
  context: 0.2,
  account: 'personal',
  group: 0,
  worktree: true,
  ...c
});

const codexChat = (c: Partial<CodexChat> & Pick<Chat, 'id' | 'name'>): CodexChat => ({
  provider: 'codex',
  status: 'resting',
  model: null,
  modelSeen: 'gpt-5.4-codex',
  effort: 'high',
  approval: 'never',
  sandbox: 'danger-full-access',
  context: 0.2,
  account: 'personal',
  group: 0,
  worktree: true,
  ...c
});

const CHATS: Chat[] = [
  chat({
    id: 'demo-1',
    name: 'Updater offered a build its own version',
    status: 'working',
    model: 'Opus',
    effort: 'max',
    context: 0.34,
    color: 'sky',
    tools: true
  }),
  chat({
    id: 'demo-2',
    name: 'Port the accounts panel onto the new store',
    status: 'waiting',
    model: 'Fable',
    effort: 'high',
    perm: 'Edits',
    context: 0.61,
    account: 'work',
    color: 'amber'
  }),
  chat({
    id: 'demo-3',
    name: 'Why does the tray icon vanish after resume?',
    status: 'resting',
    model: 'Sonnet',
    effort: 'medium',
    perm: 'Ask',
    context: 0.12,
    worktree: false,
    color: 'teal'
  }),
  codexChat({
    id: 'demo-4',
    name: 'Draft the 0.3 release notes',
    status: 'working',
    effort: 'medium',
    approval: 'on-request',
    sandbox: 'workspace-write',
    context: 0.08,
    worktree: false,
    color: 'magenta'
  })
];

const FOLDERS: Folder[] = [
  { id: 'f-1', path: 'C:\\src\\luna', open: true, picked: true, chats: CHATS.slice(0, 3) },
  { id: 'f-2', path: 'C:\\src\\notes', open: true, picked: true, chats: CHATS.slice(3) }
];

const claudeLimits = (h5: number, week: number, fable: number, h5In: number, weekIn: number): Account['limits'] => [
  { id: 'h5', label: '5 hours', short: '5H', used: h5, resetAt: hoursFromNow(h5In) },
  { id: 'week', label: 'week', short: 'W', used: week, resetAt: hoursFromNow(weekIn) },
  { id: 'scoped:Fable', label: 'Fable · week', short: 'F', used: fable, resetAt: hoursFromNow(weekIn) }
];

const account = (a: Partial<Account> & Pick<Account, 'name'>): Account => ({
  provider: 'claude',
  path: `C:\\src\\luna-accounts\\anthropic\\${a.name}`,
  plan: 'Max 20×',
  email: null,
  signedIn: true,
  haveUsage: true,
  limits: claudeLimits(0.12, 0.41, 0.27, 3.6, 96),
  usageAge: 'just now',
  fetchedAt: Date.now(),
  sync: 'ready',
  refreshError: null,
  ...a
});

const ACCOUNTS: Account[] = [
  account({ name: 'personal' }),
  account({
    name: 'work',
    plan: 'Max 5×',
    limits: claudeLimits(0.63, 0.78, 0.44, 0.9, 50)
  }),
  account({
    name: 'spare',
    plan: '—',
    signedIn: false,
    haveUsage: false,
    limits: [],
    usageAge: null,
    fetchedAt: null,
    sync: 'ready'
  }),
  account({
    provider: 'codex',
    name: 'personal',
    path: 'C:\\src\\luna-accounts\\openai\\personal',
    plan: 'Pro',
    limits: [
      { id: 'primary', label: '5 hours', short: '5H', used: 0.22, resetAt: hoursFromNow(2.2) },
      { id: 'secondary', label: 'week', short: 'W', used: 0.57, resetAt: hoursFromNow(70) }
    ]
  })
];

const slots = (...ids: Array<string | null>): Slots =>
  [0, 1, 2, 3].map(i => ids[i] ?? null);

const SPLITS = { col: 0.5, rowL: 0.5, rowR: 0.5 };

const filled = (layout: Layout): Group => ({
  layout,
  boards: {
    1: slots('demo-1'),
    2: slots('demo-1', 'demo-2'),
    3: slots('demo-1', 'demo-2', 'demo-3'),
    4: slots('demo-1', 'demo-2', 'demo-3', 'demo-4')
  },
  splitsByLayout: { 1: { ...SPLITS }, 2: { ...SPLITS }, 3: { ...SPLITS, col: 0.54 }, 4: { ...SPLITS } }
});

const parked = (): Group => ({
  layout: 1,
  boards: { 1: slots(), 2: slots(), 3: slots(), 4: slots() },
  splitsByLayout: { 1: { ...SPLITS }, 2: { ...SPLITS }, 3: { ...SPLITS }, 4: { ...SPLITS } }
});

// -------------------------------------------------------------- terminals --

const E = '\u001b[';
const dim = (s: string) => `${E}2m${s}${E}0m`;
const c = (n: number, s: string) => `${E}${n}m${s}${E}0m`;
const bold = (s: string) => `${E}1m${s}${E}0m`;

const header = (model: string, effort: string, cwd: string) =>
  [
    `${c(35, '  ▄▀▄▀▄  ')}${bold('Claude Code')} ${dim('v2.1.252')}`,
    `${c(35, '  █ ▄ █  ')}${model} with ${effort} effort ${dim('·')} Claude Max`,
    `${c(35, '  ▀▄▄▄▀  ')}${dim(cwd)}`,
    ''
  ].join('\r\n');

const codexHeader = (model: string, effort: string, cwd: string) =>
  [
    `${c(36, '>_')} ${bold('OpenAI Codex')} ${dim('(v0.154.0)')}`,
    '',
    `${dim('model:')}     ${model} ${dim(`(${effort})`)}`,
    `${dim('directory:')} ${cwd}`,
    `${dim('approval:')}  on-request ${dim('·')} ${dim('sandbox:')} workspace-write`,
    ''
  ].join('\r\n');

const user = (s: string) => `${c(36, '›')} ${s}\r\n`;
const bullet = (s: string) => `${c(35, '●')} ${s}\r\n`;
const step = (s: string) => `  ${dim('└')} ${dim(s)}\r\n`;

/** One invented session per pane, close enough to the real thing to read as one. */
const SCREENS: Record<string, string> = {
  'demo-1':
    header('Opus 5', 'max', 'C:\\src\\luna') +
    '\r\n' +
    user('the 0.2.0 update shipped the 0.1.0 setup — find out why\r\n') +
    '\r\n' +
    bullet('The bundle directory keeps every installer ever built, and') +
    '  release.ps1 took the first one alphabetically.\r\n' +
    step('Read release.ps1, listed bundle/nsis (7 files)') +
    '\r\n' +
    bullet('Picking by version instead:') +
    '\r\n' +
    `  ${c(31, '- $setup = Get-ChildItem $dir | Select-Object -First 1')}\r\n` +
    `  ${c(32, '+ $setup = Get-ChildItem $dir -Filter "*_$($version)_*-setup.exe"')}\r\n` +
    '\r\n' +
    `${c(33, '✻')} Running the release dry run… ${dim('(42s · ↓ 8.1k tokens)')}\r\n`,

  'demo-2':
    header('Fable 5', 'high', 'C:\\src\\luna') +
    '\r\n' +
    user('move the limits polling into the store\r\n') +
    '\r\n' +
    bullet('Moved the cadence into accounts.store: one timer for every') +
    '  account, backoff shared, no per-row intervals.\r\n' +
    step('Edited accounts.store.ts +48 −31, AccountsPanel.tsx +6 −22') +
    '\r\n' +
    `${c(33, '?')} ${bold('Delete the old useLimits hook?')} Nothing imports it any more.\r\n` +
    '\r\n' +
    `  ${c(32, '❯ 1. Yes, delete it')}\r\n` +
    `    2. Keep it for now\r\n` +
    '\r\n' +
    dim('  esc to interrupt · ← for agents') +
    '\r\n',

  'demo-3':
    header('Sonnet 5', 'medium', 'C:\\src\\notes') +
    '\r\n' +
    user('why does the tray icon vanish when the pc wakes up?\r\n') +
    '\r\n' +
    bullet('Explorer restarts its shell on resume and the icon goes with') +
    '  it — Windows only re-adds icons for apps that listen for\r\n' +
    '  TaskbarCreated and register again.\r\n' +
    step('Searched 3 files, read lib.rs') +
    '\r\n' +
    bullet('Nothing here listens for it yet, so the icon is gone until') +
    '  the app restarts. Want me to add the handler?\r\n' +
    '\r\n' +
    dim('  ready') +
    '\r\n',

  'demo-4':
    codexHeader('gpt-5.4-codex', 'medium', 'C:\\src\\notes') +
    '\r\n' +
    `${c(36, '›')} draft release notes for 0.3 from the log since 0.2.16\r\n` +
    '\r\n' +
    `${c(35, '•')} ${bold('Reading 41 commits')}\r\n` +
    `  ${dim('$ git log --oneline 0.2.16..HEAD')}\r\n` +
    '\r\n' +
    `  ${bold('Luna 0.3')}\r\n` +
    '  • Chats keep their own colour across panes and the sidebar\r\n' +
    '  • Window groups I–IV, each with its own four boards\r\n' +
    '  • The app ships and updates its own copies of both CLIs\r\n' +
    '\r\n' +
    `${c(33, '•')} Working ${dim('(8s · 1.2k tokens used)')}\r\n`
};

/** Paints one pane's invented session. Called from Terminal only in dev. */
export function paint(term: XTerm, chatId: string) {
  const screen = SCREENS[chatId];
  if (screen) term.write(screen);
}

// ------------------------------------------------------------- ipc stand-in --

const CLI: Record<Provider, CliStatusDto> = {
  claude: {
    provider: 'claude',
    phase: 'idle',
    version: '2.1.252',
    path: 'C:\\src\\luna\\claude-cli\\versions\\2.1.252\\claude.exe',
    latest: '2.1.252',
    got: 0,
    total: null,
    error: null,
    checkedAtMs: Date.now() - 42 * 60_000
  },
  codex: {
    provider: 'codex',
    phase: 'idle',
    version: '0.154.0',
    path: 'C:\\src\\luna\\codex-cli\\versions\\0.154.0\\bin\\codex.exe',
    latest: '0.154.0',
    got: 0,
    total: null,
    error: null,
    checkedAtMs: Date.now() - 42 * 60_000
  }
};

const ROOT: AccountsRootInfo = { path: 'C:\\src\\luna-accounts', isDefault: true };

// The chip in the footer: two sessions at work, the machine held awake.
const POWER: PowerStateDto = {
  keepAwake: true,
  holding: true,
  armed: null,
  countdownEndsAtMs: null,
  summary: { busy: 2, waiting: 1, stalled: 0, idleSinceMs: null, sessions: [] }
};

// What the new-chat dialog opens on. Left unanswered it would show every
// control sitting on Luna's own fallback, which is the one state the shot is
// least likely to be taken in and says nothing about where defaults come from.
const DEFAULTS: ClaudeDefaultsDto = {
  model: { value: 'opus', source: 'account' },
  effort: { value: 'xhigh', source: 'account' },
  permissionMode: { value: 'bypassPermissions', source: 'project' }
};

const LEVELS: Record<string, string> = {
  low: 'Fast responses with lighter reasoning',
  medium: 'Balances speed and reasoning depth for everyday tasks',
  high: 'Greater reasoning depth for complex problems',
  xhigh: 'Extra high reasoning depth for complex problems',
  max: 'Maximum reasoning depth for the hardest problems'
};
const codexModel = (slug: string, displayName: string, defaultEffort: string): CodexModelDto => ({
  slug, displayName, defaultEffort, efforts: Object.keys(LEVELS), effortHints: LEVELS
});
const CODEX_MODELS: CodexModelDto[] = [
  codexModel('gpt-6.1-sol', 'GPT-6.1-Sol', 'low'),
  codexModel('gpt-6-astra', 'GPT-6-Astra', 'medium'),
  codexModel('gpt-6-sol', 'GPT-6-Sol', 'medium'),
  codexModel('gpt-6-luna', 'GPT-6-Luna', 'medium'),
  codexModel('gpt-5.6-terra', 'GPT-5.6-Terra', 'medium')
];

const CODEX_DEFAULTS: CodexDefaultsDto = {
  model: { value: 'gpt-6.1-sol', source: 'codex' },
  effort: { value: 'low', source: 'codex' },
  approval: { value: 'never', source: 'stock' },
  sandbox: { value: 'workspace-write', source: 'project' }
};

let seeded = false;

/**
 * Answers the few commands whose emptiness would show. Without this the
 * settings dialog draws no CLI version and no accounts folder in a browser,
 * and a screenshot of it is a picture of blanks. Everything else keeps the
 * caller's own fallback, and outside `?demo` this does nothing at all.
 */
const RECOMMENDED = [
  { key: 'tui', current: '"fullscreen"', recommended: '"fullscreen"', why: 'Draws on the alternate screen with its own scrollback.', defaultOn: true, applied: true },
  { key: 'spinnerTipsEnabled', current: null, recommended: 'false', why: 'No tips under the spinner.', defaultOn: true, applied: false },
  { key: 'copyOnSelect', current: null, recommended: 'false', why: 'Selecting in a pane copies nothing until Ctrl+C, so the clipboard keeps what you put there.', defaultOn: true, applied: false, file: '.claude.json' },
  { key: 'attribution', current: null, recommended: '{"commit":"","pr":"","sessionUrl":false}', why: 'No Co-Authored-By or session links in commits and PRs.', defaultOn: false, applied: false }
];

// Worktrees left behind with no chat in them, for the Worktrees window: one
// clean and one holding work under each CLI, and one more in the second folder.
const STALE: Record<string, WorktreeInfoDto[]> = {
  'C:\\src\\luna': [
    { path: 'C:\\src\\luna\\.claude\\worktrees\\merry-coalescing-cosmos', branch: 'worktree-merry-coalescing-cosmos', uncommitted: 0, uniqueCommits: 0, touchedMs: Date.now() - 3 * 86_400_000, broken: false },
    { path: 'C:\\src\\luna\\.claude\\worktrees\\piped-squishing-blum', branch: 'fix-updater', uncommitted: 3, uniqueCommits: 2, touchedMs: Date.now() - 5 * 3_600_000, broken: false },
    { path: 'C:\\src\\luna-accounts\\openai\\personal\\worktrees\\d6ff\\luna', branch: null, uncommitted: 0, uniqueCommits: 0, touchedMs: Date.now() - 9 * 86_400_000, broken: false }
  ],
  'C:\\src\\notes': [
    { path: 'C:\\src\\luna-accounts\\openai\\personal\\worktrees\\0a1b\\notes', branch: null, uncommitted: 1, uniqueCommits: 0, touchedMs: Date.now() - 40 * 60_000, broken: false }
  ]
};

// The library: two skills laid out — one into every account, one into a
// single account — and a draft that does not do yet; instructions for
// everyone, others for the work account alone (edited by hand there since
// Luna wrote them), a blank draft, and a Codex account kept on its own file.
const SHARED = 'C:\\src\\luna\\.shared-skills';
const accountsDir = (vendor: string, name: string) => `C:\\Users\\me\\Documents\\luna-accounts\\${vendor}\\${name}`;
const place = (p: Partial<PlaceDto> & Pick<PlaceDto, 'dest' | 'target'>): PlaceDto => ({
  skill: null, instructions: null, status: 'current', both: false, problem: null, ...p
});
const LIBRARY: LibraryOverviewDto = {
  root: 'C:\\Users\\me\\AppData\\Local\\luna\\library',
  isDefault: true,
  found: true,
  instructions: [
    { name: 'main', description: 'Working with me', problem: null, accounts: ['all'] },
    { name: 'terse', description: '', problem: 'Blank: laid out nowhere until it has something in it', accounts: [] },
    { name: 'work', description: 'At work: the monorepo, its CI and who reviews what', problem: null, accounts: ['claude/work'] }
  ],
  noInstructions: ['codex/team'],
  instructionsFor: { 'claude/personal': 'main', 'claude/work': 'work', 'codex/personal': 'main' },
  oldInstructions: null,
  skills: [
    { name: 'luna-review', description: 'Hand a finished change to a model of the other CLI through Luna and fold its findings back in.', problem: null, accounts: ['all'] },
    { name: 'migrations', description: '', problem: 'The front matter has no description', accounts: ['claude'] },
    { name: 'release-notes', description: 'Write the release notes from the commits since the last tag, in the house style.', problem: null, accounts: ['claude/work'] }
  ],
  accounts: [
    { provider: 'claude', name: 'personal' },
    { provider: 'claude', name: 'work' },
    { provider: 'codex', name: 'personal' },
    { provider: 'codex', name: 'team' }
  ],
  folders: [{ path: SHARED, skills: ['luna-review', 'release-notes'], found: true }],
  places: [
    place({ dest: `${accountsDir('anthropic', 'personal')}\\CLAUDE.md`, target: { kind: 'account', provider: 'claude', name: 'personal' }, instructions: 'main' }),
    place({ dest: `${accountsDir('anthropic', 'work')}\\CLAUDE.md`, target: { kind: 'account', provider: 'claude', name: 'work' }, instructions: 'work', status: 'changed' }),
    place({ dest: `${accountsDir('openai', 'personal')}\\AGENTS.md`, target: { kind: 'account', provider: 'codex', name: 'personal' }, instructions: 'main' }),
    ...[['anthropic', 'claude', 'personal'], ['anthropic', 'claude', 'work'], ['openai', 'codex', 'personal'], ['openai', 'codex', 'team']].map(([v, p, n]) =>
      place({ dest: `${accountsDir(v, n)}\\skills\\luna-review`, target: { kind: 'account', provider: p as Provider, name: n }, skill: 'luna-review' })
    ),
    place({ dest: `${accountsDir('anthropic', 'work')}\\skills\\release-notes`, target: { kind: 'account', provider: 'claude', name: 'work' }, skill: 'release-notes' }),
    ...['personal', 'work'].map(n =>
      place({
        dest: `${accountsDir('anthropic', n)}\\skills\\migrations`, target: { kind: 'account', provider: 'claude', name: n },
        skill: 'migrations', status: 'failed', problem: 'The front matter has no description'
      })
    ),
    place({ dest: `${SHARED}\\luna-review`, target: { kind: 'folder', path: SHARED }, skill: 'luna-review' }),
    place({ dest: `${SHARED}\\release-notes`, target: { kind: 'folder', path: SHARED }, skill: 'release-notes', status: 'taken' })
  ],
  foundInstructions: [{ dest: `${accountsDir('openai', 'team')}\\AGENTS.md`, target: { kind: 'account', provider: 'codex', name: 'team' } }],
  overridden: [],
  kept: []
};
const LIBRARY_DIFF: FileDiffDto[] = [
  {
    path: 'CLAUDE.md',
    base: '# Working with me\n\n- Answer in the language I write in.\n- Run the tests before saying done.\n- Commit only when asked.\n',
    library: '# Working with me\n\n- Answer in the language I write in.\n- Run the tests before saying done.\n- Commit only when asked.\n',
    there: '# Working with me\n\n- Answer in the language I write in.\n- Run the tests before saying done, and say which.\n- Commit only when asked.\n- Never push to master.\n',
    binary: false
  }
];

export function answer<T>(cmd: string, args: Record<string, unknown>, fallback: T): T {
  if (!seeded) return fallback;
  if (cmd === 'library_overview' || cmd === 'library_sync') return LIBRARY as unknown as T;
  if (cmd === 'library_diff') return LIBRARY_DIFF as unknown as T;
  if (cmd === 'orphan_worktrees') return (STALE[args.folder as string] ?? []).map(w => w.path) as unknown as T;
  if (cmd === 'inspect_worktrees') return (STALE[args.folder as string] ?? []) as unknown as T;
  if (cmd === 'cli_status') return (CLI[args.provider as Provider] ?? CLI.claude) as unknown as T;
  if (cmd === 'get_accounts_root') return ROOT as unknown as T;
  if (cmd === 'claude_defaults') return DEFAULTS as unknown as T;
  if (cmd === 'codex_defaults') return CODEX_DEFAULTS as unknown as T;
  if (cmd === 'codex_models') return CODEX_MODELS as unknown as T;
  if (cmd === 'power_state') return POWER as unknown as T;
  if (cmd === 'background_helpers') return BACKGROUND as unknown as T;
  // The Accounts window's own dialogs, so they can be looked at too.
  if (cmd === 'read_account_config') {
    const claudeSide = args.provider === 'claude';
    return {
      path: `${args.accountPath}\\${claudeSide ? 'settings.json' : 'config.toml'}`,
      text: claudeSide
        ? '{\n  "tui": "fullscreen",\n  "spinnerTipsEnabled": false,\n  "env": {\n    "CLAUDE_CODE_DISABLE_FEEDBACK_SURVEY": "1"\n  }\n}\n'
        : '# Luna updates Codex itself\ncheck_for_update_on_startup = false\n\n[tui]\nshow_tooltips = false\n',
      exists: true
    } as unknown as T;
  }
  if (cmd === 'recommended_settings') return RECOMMENDED as unknown as T;
  if (cmd === 'check_account_config') return null as unknown as T;
  if (cmd === 'settings_sync_state') return { claude: 'personal', codex: null, onStart: true } as unknown as T;
  return fallback;
}

// ------------------------------------------------------------------ seed --

// Two helpers the first chat's agent runs in the background, one of them
// waiting on an answer: the chip on its row.
const BACKGROUND: BackgroundHelperDto[] = [
  {
    id: 'demo-bg-1', parent: 'demo-1', name: 'Review the release plan', provider: 'codex', state: 'working',
    account: 'work', model: 'gpt-5.5', effort: 'high', worktree: true,
    startedMs: Date.now() - 14 * 60_000, turnStartedMs: Date.now() - 4 * 60_000
  },
  {
    id: 'demo-bg-2', parent: 'demo-1', name: 'Check the changelog', provider: 'claude', state: 'question',
    account: 'team', model: 'sonnet', effort: 'medium', worktree: false,
    startedMs: Date.now() - 6 * 60_000, turnStartedMs: Date.now() - 60_000,
    ask: { kind: 'question', blocking: true, questions: [{ text: 'Ship tonight or tomorrow?', options: ['Tonight', 'Tomorrow'] }] }
  }
];

export function seedDemo(scene: Scene) {
  seeded = true;
  useBackground.setState({ helpers: BACKGROUND });
  // The shell store has read the theme by now; tell it, not localStorage.
  useShell.setState({ theme: scene === 'dark' ? 'dark' : 'light', pinned: true });
  // The version readout, which in a browser has no app to ask.
  useUpdates.setState({ current: '0.6.0', checkedAt: Date.now() - 12 * 60_000 });

  // The app polls for the account list and for each chat's status on a timer, and
  // in a browser both come back empty — which wiped the fixture a second after
  // it landed, leaving four panes saying the account was not on disk. Silence
  // the writers rather than the pollers: less to keep in step with.
  useChats.setState({
    folders: FOLDERS,
    active: 'demo-1',
    setStatus: () => {},
    setName: () => {},
    setContext: () => {}
  });
  // A draft left in the first chat, and a message from another session it
  // holds up: the tag and the balloon both show.
  useDrafts.setState({ by: { 'demo-1': { draft: true, held: 1 } }, set: () => {} });
  useAccounts.setState({
    accounts: ACCOUNTS,
    loaded: true,
    refresh: async () => {},
    startPolling: () => {},
    stopPolling: () => {}
  });
  // The dark shot is a two-pane board, so the two arrangements the app is
  // actually used in both end up in the README.
  usePanes.setState({
    group: 0,
    groups: [filled(scene === 'dark' ? 2 : scene === 'peek' ? 3 : 4), parked(), parked(), parked()],
    // The peek scene is that sheet and nothing else: the second pane of three,
    // held up over a board that is still there behind it.
    peek: scene === 'peek' ? 1 : null
  });

  if (scene === 'codex') useNewChat.setState({ lastAccount: { provider: 'codex', name: 'personal' } });
  if (scene === 'newchat' || scene === 'codex') useNewChat.getState().openDialog('C:\\src\\luna');
  if (scene === 'accounts' || scene === 'recommended' || scene === 'editor') useAccountsWindow.getState().setOpen(true);
  if (scene === 'worktrees') useWorktrees.getState().setOpen(true);
  if (scene === 'library') {
    useLibrary.getState().setOpen(true);
    // The changed copy's differences, opened the way a click would.
    setTimeout(() => [...document.querySelectorAll<HTMLButtonElement>('button')].find(b => b.textContent === 'Differences')?.click(), 600);
  }
  // The two dialogs belong to an account row; open them the way a click would.
  const button = scene === 'recommended' ? 'Recommended settings' : scene === 'editor' ? 'Edit settings.json' : null;
  if (button) {
    setTimeout(() => document.querySelector<HTMLElement>(`button[aria-label^="${button}"]`)?.click(), 300);
  }
}

/** `?demo` / `?demo=dark` — which fixture the URL is asking for, if any. */
export function sceneFromUrl(search: string): Scene | null {
  const value = new URLSearchParams(search).get('demo');
  if (value === null) return null;
  return SCENES.find(s => s === value) ?? 'main';
}
