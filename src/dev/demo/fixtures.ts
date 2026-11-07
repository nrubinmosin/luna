// The invented chats, accounts and answers the demo scenes stand on.
import type { Account, Chat, ClaudeChat, CodexChat, Folder, Provider } from '../../shared/types';
import type { AccountsRootInfo } from '../../ipc/accounts';
import type { BackgroundHelperDto } from '../../ipc/agents';
import type { CliStatusDto } from '../../ipc/cli';
import type { CoreStateDto } from '../../ipc/coreProcess';
import type { ClaudeDefaultsDto, CodexDefaultsDto, CodexModelDto } from '../../ipc/defaults';
import type { FileDiffDto, LibraryOverviewDto, PlaceDto } from '../../ipc/library';
import type { PowerStateDto } from '../../ipc/power';
import type { WorktreeInfoDto } from '../../ipc/worktrees';
import type { Group, Layout, Slots } from '../../features/panes/panes.store';

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

export const FOLDERS: Folder[] = [
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

export const ACCOUNTS: Account[] = [
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

const slots = (...ids: Array<string | null>): Slots => [0, 1, 2, 3].map(i => ids[i] ?? null);

const SPLITS = { col: 0.5, rowL: 0.5, rowR: 0.5 };

export const filled = (layout: Layout): Group => ({
  layout,
  boards: {
    1: slots('demo-1'),
    2: slots('demo-1', 'demo-2'),
    3: slots('demo-1', 'demo-2', 'demo-3'),
    4: slots('demo-1', 'demo-2', 'demo-3', 'demo-4')
  },
  splitsByLayout: { 1: { ...SPLITS }, 2: { ...SPLITS }, 3: { ...SPLITS, col: 0.54 }, 4: { ...SPLITS } }
});

export const parked = (): Group => ({
  layout: 1,
  boards: { 1: slots(), 2: slots(), 3: slots(), 4: slots() },
  splitsByLayout: { 1: { ...SPLITS }, 2: { ...SPLITS }, 3: { ...SPLITS }, 4: { ...SPLITS } }
});

export const CLI: Record<Provider, CliStatusDto> = {
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

export const ROOT: AccountsRootInfo = { path: 'C:\\src\\luna-accounts', isDefault: true };

// The chip in the footer: two sessions at work, the machine held awake.
export const POWER: PowerStateDto = {
  keepAwake: true,
  holding: true,
  armed: null,
  countdownEndsAtMs: null,
  summary: { busy: 2, waiting: 1, stalled: 0, idleSinceMs: null, sessions: [] }
};

// What the new-chat dialog opens on. Left unanswered it would show every
// control sitting on Luna's own fallback, which is the one state the shot is
// least likely to be taken in and says nothing about where defaults come from.
export const DEFAULTS: ClaudeDefaultsDto = {
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
  slug,
  displayName,
  defaultEffort,
  efforts: Object.keys(LEVELS),
  effortHints: LEVELS
});
export const CODEX_MODELS: CodexModelDto[] = [
  codexModel('gpt-6.1-sol', 'GPT-6.1-Sol', 'low'),
  codexModel('gpt-6-astra', 'GPT-6-Astra', 'medium'),
  codexModel('gpt-6-sol', 'GPT-6-Sol', 'medium'),
  codexModel('gpt-6-luna', 'GPT-6-Luna', 'medium'),
  codexModel('gpt-5.6-terra', 'GPT-5.6-Terra', 'medium')
];

export const CODEX_DEFAULTS: CodexDefaultsDto = {
  model: { value: 'gpt-6.1-sol', source: 'codex' },
  effort: { value: 'low', source: 'codex' },
  approval: { value: 'never', source: 'stock' },
  sandbox: { value: 'workspace-write', source: 'project' }
};

const CHECK = {
  account: null,
  status: 'off',
  current: null,
  target: null,
  risk: null,
  note: null,
  action: null,
  refused: null,
  dismissed: false
};
export const CHECKS = [
  {
    ...CHECK,
    id: 'claude-policy-hooks',
    title: 'Hooks are limited by policy',
    status: 'blocked',
    current: 'allowManagedHooksOnly',
    why: "A policy on this computer keeps Claude Code from running Luna's hooks, on every account.",
    note: 'Set by your administrator: nothing in Luna or an account can change it.'
  },
  {
    ...CHECK,
    id: 'codex-hooks-unasked',
    title: 'Codex hooks without asking is off',
    current: 'off',
    target: 'on',
    action: 'Turn on',
    why: "Codex drops a hook it has not been told to trust, so Luna's guard against endless waits never reaches Codex chats and helpers.",
    risk: "This runs every project's hooks without review in every Codex chat and helper, not only Luna's guard: a repository you open can run what it likes on this computer."
  },
  {
    ...CHECK,
    id: 'claude-hooks',
    account: 'claude/personal',
    title: 'Hooks are switched off',
    current: 'disableAllHooks: true',
    target: 'key removed',
    action: 'Turn hooks on',
    why: "Claude Code runs no hook on this account, and Luna's are how it hears of a turn's end, a question and a subagent; the guard against endless waits goes with them.",
    risk: "Every hook of this account comes back, not only Luna's: any written in its settings that you have not reviewed runs at its next event. This is the accounts' settings parent. The next settings sync copies it to work."
  },
  {
    ...CHECK,
    id: 'claude-fullscreen',
    account: 'claude/work',
    title: 'Fullscreen was turned off',
    current: 'turned off on 2.1.318',
    target: 'cleared',
    action: 'Try fullscreen again',
    why: 'Claude Code gave up on its fullscreen renderer here after launches that did not finish, so panes flicker and redraw badly on resize.',
    note: "Written only while no session of Luna's is on the account. A Claude Code running outside Luna can still race this write.",
    refused: 'work is in use. Stop its sessions; one closing counts until its process has exited.'
  },
  {
    ...CHECK,
    id: 'keep-awake',
    title: 'Keep awake is off',
    current: 'off',
    target: 'on',
    action: 'Turn on',
    dismissed: true,
    why: 'The PC can go to sleep under a running chat or helper, and the work stops with it.'
  },
  { ...CHECK, id: 'waiting-marker', title: 'Waiting for you', status: 'ok', why: '' }
];

export const RECOMMENDED = [
  {
    key: 'tui',
    current: '"fullscreen"',
    recommended: '"fullscreen"',
    why: 'Draws on the alternate screen with its own scrollback.',
    defaultOn: true,
    applied: true
  },
  {
    key: 'spinnerTipsEnabled',
    current: null,
    recommended: 'false',
    why: 'No tips under the spinner.',
    defaultOn: true,
    applied: false
  },
  {
    key: 'copyOnSelect',
    current: null,
    recommended: 'false',
    why: 'Selecting in a pane copies nothing until Ctrl+C, so the clipboard keeps what you put there.',
    defaultOn: true,
    applied: false,
    file: '.claude.json'
  },
  {
    key: 'attribution',
    current: null,
    recommended: '{"commit":"","pr":"","sessionUrl":false}',
    why: 'No Co-Authored-By or session links in commits and PRs.',
    defaultOn: false,
    applied: false
  }
];

// Worktrees left behind with no chat in them, for the Worktrees window: one
// clean and one holding work under each CLI, and one more in the second folder.
export const STALE: Record<string, WorktreeInfoDto[]> = {
  'C:\\src\\luna': [
    {
      path: 'C:\\src\\luna\\.claude\\worktrees\\merry-coalescing-cosmos',
      branch: 'worktree-merry-coalescing-cosmos',
      uncommitted: 0,
      uniqueCommits: 0,
      touchedMs: Date.now() - 3 * 86_400_000,
      broken: false
    },
    {
      path: 'C:\\src\\luna\\.claude\\worktrees\\piped-squishing-blum',
      branch: 'fix-updater',
      uncommitted: 3,
      uniqueCommits: 2,
      touchedMs: Date.now() - 5 * 3_600_000,
      broken: false
    },
    {
      path: 'C:\\src\\luna-accounts\\openai\\personal\\worktrees\\d6ff\\luna',
      branch: null,
      uncommitted: 0,
      uniqueCommits: 0,
      touchedMs: Date.now() - 9 * 86_400_000,
      broken: false
    }
  ],
  'C:\\src\\notes': [
    {
      path: 'C:\\src\\luna-accounts\\openai\\personal\\worktrees\\0a1b\\notes',
      branch: null,
      uncommitted: 1,
      uniqueCommits: 0,
      touchedMs: Date.now() - 40 * 60_000,
      broken: false
    }
  ]
};

// The library: two skills laid out — one into every Claude Code account and to
// the Codex sessions in a repository, one only for the work account's sessions
// there, under another name as the repository has its own — and a draft that
// does not do yet, turned off in the repository; instructions for everyone, others for the work account
// alone (edited by hand there since Luna wrote them), a blank draft, and a
// Codex account kept on its own file.
const REPO = 'C:\\src\\asuri-app';
const WAITING = 'C:\\Users\\me\\AppData\\Local\\luna\\library-folders\\asuri-app';
const accountsDir = (vendor: string, name: string) => `C:\\Users\\me\\Documents\\luna-accounts\\${vendor}\\${name}`;
const place = (p: Partial<PlaceDto> & Pick<PlaceDto, 'dest' | 'target'>): PlaceDto => ({
  skill: null,
  instructions: null,
  shownAs: null,
  status: 'current',
  both: false,
  problem: null,
  ...p
});
export const LIBRARY: LibraryOverviewDto = {
  root: 'C:\\Users\\me\\AppData\\Local\\luna\\library',
  isDefault: true,
  found: true,
  instructions: [
    { name: 'main', description: 'Working with me', problem: null, accounts: ['all'] },
    { name: 'terse', description: '', problem: 'Blank: laid out nowhere until it has something in it', accounts: [] },
    {
      name: 'work',
      description: 'At work: the monorepo, its CI and who reviews what',
      problem: null,
      accounts: ['claude/work']
    }
  ],
  noInstructions: ['codex/team'],
  instructionsFor: { 'claude/personal': 'main', 'claude/work': 'work', 'codex/personal': 'main' },
  oldInstructions: null,
  skills: [
    {
      name: 'luna-review',
      description: 'Hand a finished change to a model of the other CLI through Luna and fold its findings back in.',
      problem: null,
      accounts: ['claude']
    },
    { name: 'migrations', description: '', problem: 'The front matter has no description', accounts: ['claude'] },
    {
      name: 'release-notes',
      description: 'Write the release notes from the commits since the last tag, in the house style.',
      problem: null,
      accounts: []
    }
  ],
  accounts: [
    { provider: 'claude', name: 'personal' },
    { provider: 'claude', name: 'work' },
    { provider: 'codex', name: 'personal' },
    { provider: 'codex', name: 'team' }
  ],
  folders: [
    {
      path: REPO,
      skills: { 'luna-review': ['all'], 'release-notes': ['claude/work', 'codex/team'], migrations: [] },
      instructions: { 'claude/work': null, 'claude/personal': 'work' },
      found: true,
      inherited: { 'luna-review': ['claude/personal', 'claude/work'], migrations: ['claude/personal', 'claude/work'] }
    }
  ],
  places: [
    place({
      dest: `${accountsDir('anthropic', 'personal')}\\CLAUDE.md`,
      target: { kind: 'account', provider: 'claude', name: 'personal' },
      instructions: 'main'
    }),
    place({
      dest: `${accountsDir('anthropic', 'work')}\\CLAUDE.md`,
      target: { kind: 'account', provider: 'claude', name: 'work' },
      instructions: 'work',
      status: 'changed'
    }),
    place({
      dest: `${accountsDir('openai', 'personal')}\\AGENTS.md`,
      target: { kind: 'account', provider: 'codex', name: 'personal' },
      instructions: 'main'
    }),
    ...['personal', 'work'].map(n =>
      place({
        dest: `${accountsDir('anthropic', n)}\\skills\\luna-review`,
        target: { kind: 'account', provider: 'claude', name: n },
        skill: 'luna-review'
      })
    ),
    ...['personal', 'work'].map(n =>
      place({
        dest: `${accountsDir('anthropic', n)}\\skills\\migrations`,
        target: { kind: 'account', provider: 'claude', name: n },
        skill: 'migrations',
        status: 'failed',
        problem: 'The front matter has no description'
      })
    ),
    ...['personal', 'team'].map(n =>
      place({
        dest: `${accountsDir('openai', n)}\\plugins\\cache\\luna\\asuri-app\\local\\skills\\luna-review`,
        target: { kind: 'folder', path: REPO, provider: 'codex', name: n },
        skill: 'luna-review'
      })
    ),
    place({
      dest: `${WAITING}\\work\\.claude\\skills\\luna-release-notes`,
      target: { kind: 'folder', path: REPO, provider: 'claude', name: 'work' },
      skill: 'release-notes',
      shownAs: 'luna-release-notes'
    })
  ],
  foundInstructions: [
    {
      dest: `${accountsDir('openai', 'team')}\\AGENTS.md`,
      target: { kind: 'account', provider: 'codex', name: 'team' }
    }
  ],
  overridden: [],
  kept: []
};
export const LIBRARY_DIFF: FileDiffDto[] = [
  {
    path: 'CLAUDE.md',
    base: '# Working with me\n\n- Answer in the language I write in.\n- Run the tests before saying done.\n- Commit only when asked.\n',
    library:
      '# Working with me\n\n- Answer in the language I write in.\n- Run the tests before saying done.\n- Commit only when asked.\n',
    there:
      '# Working with me\n\n- Answer in the language I write in.\n- Run the tests before saying done, and say which.\n- Commit only when asked.\n- Never push to master.\n',
    binary: false
  }
];

// A browser has no core to ask, and no answer reads as one that stopped: the
// banner saying so would be across the top of every shot.
export const CORE: CoreStateDto = {
  state: 'up',
  core: { pid: 1, version: '0.6.0', build: 'demo', fingerprint: 'demo', started: 0, sessions: 4 },
  error: null
};

// Two helpers the first chat's agent runs in the background, one of them
// waiting on an answer: the chip on its row.
export const BACKGROUND: BackgroundHelperDto[] = [
  {
    id: 'demo-bg-1',
    parent: 'demo-1',
    name: 'Review the release plan',
    provider: 'codex',
    state: 'working',
    account: 'work',
    model: 'gpt-5.5',
    effort: 'high',
    worktree: true,
    startedMs: Date.now() - 14 * 60_000,
    turnStartedMs: Date.now() - 4 * 60_000
  },
  {
    id: 'demo-bg-2',
    parent: 'demo-1',
    name: 'Check the changelog',
    provider: 'claude',
    state: 'question',
    account: 'team',
    model: 'sonnet',
    effort: 'medium',
    worktree: false,
    startedMs: Date.now() - 6 * 60_000,
    turnStartedMs: Date.now() - 60_000,
    ask: {
      kind: 'question',
      blocking: true,
      questions: [{ text: 'Ship tonight or tomorrow?', options: ['Tonight', 'Tomorrow'] }]
    }
  },
  {
    id: 'demo-bg-3',
    parent: null,
    name: 'Sweep old branches',
    provider: 'claude',
    state: 'idle',
    account: 'team',
    model: 'haiku',
    effort: 'low',
    worktree: false,
    startedMs: Date.now() - 40 * 60_000,
    turnStartedMs: null
  }
];
