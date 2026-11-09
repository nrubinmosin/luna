// The invented chats, accounts and answers the demo scenes stand on.
import type { Account, Chat, ClaudeChat, CodexChat, Folder, Provider } from '../../shared/types';
import type { AccountsRootInfo } from '../../ipc/accounts';
import type { AtStakeDto, BackgroundHelperDto, HelperTranscriptDto } from '../../ipc/agents';
import type { CliStatusDto, CliVersionsDto } from '../../ipc/cli';
import type { CoreStateDto } from '../../ipc/coreProcess';
import type { ClaudeDefaultsDto, CodexDefaultsDto, CodexModelDto } from '../../ipc/defaults';
import type { FileDiffDto, LibraryOverviewDto, PlaceDto } from '../../ipc/library';
import type { PowerStateDto } from '../../ipc/power';
import type { OrphanSessionDto } from '../../ipc/sessions';
import type { BrowsersDto } from '../../ipc/browsers';
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

/** The same, the first chat set to hand off at 70% and gone on past its
 *  second handoff a little while ago. */
export const HANDED_OFF: Folder[] = FOLDERS.map(f => ({
  ...f,
  chats: f.chats.map(c =>
    c.id === 'demo-1'
      ? {
          ...c,
          context: 0.09,
          autoHandoff: true,
          handoffAt: 70,
          handoff: { n: 2, at: Date.now() - 42 * 60_000, state: 'done' as const, previous: true }
        }
      : c
  )
}));

/** Luna's copy of that handoff. */
export const HANDOFF_FILE = `# Handoff 2: the updater offers a build its own version

## Objective, scope, stop conditions
Stop the updater offering the running build as an update. Scope: src-tauri/src/updater and its tests.
Stop and ask before touching the release workflow.

## Done, and what remains
- Found it: the version compare ignores the build stamp.
- Remaining: the fix, a unit test that fails without it, the gate.

## Decisions
- Compare by the full version with its stamp; rejected: a second manifest field (needs a release).

## Helpers
- cmw3kq1 "tests": runs the suite on the fix; its report comes as a notice.

## Next action
Write the test in updater/check.rs, then the fix.
`;

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
    channel: 'latest',
    got: 0,
    total: null,
    error: null,
    note: null,
    checkedAtMs: Date.now() - 42 * 60_000
  },
  codex: {
    provider: 'codex',
    phase: 'idle',
    version: '0.154.0',
    path: 'C:\\src\\luna\\codex-cli\\versions\\0.154.0\\bin\\codex.exe',
    latest: '0.155.0',
    channel: 'knownGood',
    got: 0,
    total: null,
    error: null,
    note: null,
    checkedAtMs: Date.now() - 42 * 60_000
  }
};

/** Settings → CLIs: Claude Code on latest, its new version's canary warning
 *  (only the CLIs scenes show it), Codex on known-good. */
export const CLI_VERSIONS: Record<Provider, CliVersionsDto> = {
  claude: {
    provider: 'claude',
    channel: 'latest',
    versions: [
      { dir: '2.1.252', version: '2.1.252', current: true, previous: false, inUse: true, knownGood: false, bad: null },
      { dir: '2.1.240', version: '2.1.240', current: false, previous: true, inUse: true, knownGood: true, bad: null }
    ],
    knownGood: ['2.1.240'],
    minSafe: '2.1.240',
    floor: '2.1.252',
    pinned: null,
    bad: {},
    canary: {
      version: '2.1.252',
      watched: 3,
      failures: ['exited with an error 2s after it started', 'exited with an error 3s after it started'],
      warned: true,
      dismissed: false
    }
  },
  codex: {
    provider: 'codex',
    channel: 'knownGood',
    versions: [
      { dir: '0.154.0', version: '0.154.0', current: true, previous: false, inUse: true, knownGood: true, bad: null },
      {
        dir: '0.153.0',
        version: '0.153.0',
        current: false,
        previous: true,
        inUse: false,
        knownGood: true,
        bad: 'failed the offline probe: `exec` no longer lists --json'
      }
    ],
    knownGood: ['0.153.0', '0.154.0'],
    minSafe: '0.150.0',
    floor: '0.154.0',
    pinned: null,
    bad: { '0.153.0': 'failed the offline probe: `exec` no longer lists --json' },
    canary: null
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

// An update asked about with eight sessions at work and a long release note:
// the tallest confirmation there is.
const busy = (id: string, procs: string[]) => ({
  id,
  turn: 'busy' as const,
  procs,
  outputFresh: true,
  busy: true,
  stalled: null
});
export const UPDATE_POWER: PowerStateDto = {
  ...POWER,
  summary: {
    ...POWER.summary,
    busy: 8,
    waiting: 0,
    sessions: [
      busy('demo-1', ['cargo test']),
      busy('demo-2', []),
      busy('demo-3', ['pnpm vitest']),
      busy('demo-4', []),
      busy('7f3a9c21-0d4e', ['git worktree add']),
      busy('b04e1d55-91aa', []),
      busy('c9d2e8a0-5b17', ['cargo clippy']),
      busy('1e6f4b73-e2c8', [])
    ]
  }
};
export const UPDATE_NOTES = Array.from(
  { length: 24 },
  (_, i) => `- Change ${i + 1}: a line on what it does for whoever runs Luna, as release notes have them.`
).join('\n');

// Sessions still running that no chat claims, as a lost state restore leaves them.
const orphan = (n: number, title: string | null, cwd: string, provider: Provider = 'claude'): OrphanSessionDto => ({
  id: `lost-${n}`,
  provider,
  pid: 14200 + n * 316,
  cwd,
  accountPath: `C:\\src\\luna-accounts\\${provider === 'claude' ? 'anthropic' : 'openai'}\\personal`,
  title,
  status: 'resting'
});
export const ORPHANS: OrphanSessionDto[] = [
  orphan(1, 'Bisect the flaky pty test', 'C:\\src\\luna'),
  orphan(2, 'Try the new Codex sandbox flag', 'C:\\src\\luna', 'codex'),
  orphan(3, null, 'C:\\src\\notes'),
  orphan(4, 'Rename the hub lanes', 'C:\\src\\luna'),
  orphan(5, 'Draft the 0.4 roadmap', 'C:\\src\\notes', 'codex'),
  orphan(6, 'Profile the scrollback replay', 'C:\\src\\luna'),
  orphan(7, 'Read the WebView2 release notes', 'C:\\src\\luna')
];

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
// Codex account kept on its own file. The instructions for everyone and the
// review skill are for the person's own chats, kept from helpers.
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
    { name: 'main', description: 'Working with me', problem: null, accounts: ['all'], audience: 'chats' },
    {
      name: 'terse',
      description: '',
      problem: 'Blank: laid out nowhere until it has something in it',
      accounts: [],
      audience: 'both'
    },
    {
      name: 'work',
      description: 'At work: the monorepo, its CI and who reviews what',
      problem: null,
      accounts: ['claude/work'],
      audience: 'both'
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
      accounts: ['claude'],
      audience: 'chats'
    },
    {
      name: 'migrations',
      description: '',
      problem: 'The front matter has no description',
      accounts: ['claude'],
      audience: 'both'
    },
    {
      name: 'release-notes',
      description: 'Write the release notes from the commits since the last tag, in the house style.',
      problem: null,
      accounts: [],
      audience: 'both'
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

// What the working helper has said so far, as its read-only view shows it.
export const HELPER_TRANSCRIPT: HelperTranscriptDto = {
  messages: [
    {
      role: 'user',
      text: 'Review docs/release-plan.md against what CHANGELOG.md says is in 0.11.0. List anything the plan promises that the changelog does not have, and the other way round.'
    },
    {
      role: 'assistant',
      text: 'Reading both files first.\n\n[tool: Read docs/release-plan.md]\n[tool: Read CHANGELOG.md]'
    },
    {
      role: 'assistant',
      text: 'Two gaps so far:\n\n1. The plan promises the read-only helper view; the changelog has it under Added.\n2. The changelog lists the delete dialog guards; the plan does not mention them.\n\nChecking the migration notes next.'
    }
  ],
  cursor: 4096,
  file: 'C:\\Users\\demo\\.codex\\sessions\\2026\\11\\09\\rollout-demo.jsonl',
  running: true
};

// What the first chat's helpers have at stake in its delete: one at work in a
// worktree holding uncommitted work, one waiting on a question.
export const AT_STAKE: Record<string, AtStakeDto> = {
  'demo-bg-1': {
    id: 'demo-bg-1',
    working: 'in the middle of a turn',
    worktree: 'C:\\src\\luna\\.codex\\worktrees\\review',
    unsaved: '2 uncommitted files'
  },
  'demo-bg-2': { id: 'demo-bg-2', working: 'waits on a question or permission' }
};

/** What the first chat's panel says Luna applied (applied.rs), as a Claude
 *  Code chat with Luna's tools started on a work account. */
export const APPLIED = {
  launch: [
    {
      key: 'account',
      class: 'runtime',
      value: 'CLAUDE_CONFIG_DIR=C:\\Users\\dev\\Luna\\accounts\\anthropic\\work',
      why: 'The account this chat belongs to: the CLI reads its settings and sign-in from there.'
    },
    {
      key: 'no-self-update',
      class: 'runtime',
      value: 'DISABLE_AUTOUPDATER=1',
      why: 'Luna installs and updates the CLI itself; the CLI’s own updater would install or announce a copy nothing here runs.'
    },
    {
      key: 'session-id',
      class: 'runtime',
      value: 'LUNA_SESSION_ID=demo-1',
      why: 'Tells the session, and what it runs, which Luna chat it is (and a helper which chat started it).'
    },
    {
      key: 'model',
      class: 'userAction',
      value: 'opus',
      why: 'As chosen for this chat: in the new-chat dialog, or from the account’s settings.'
    },
    {
      key: 'effort',
      class: 'userAction',
      value: 'max',
      why: 'As chosen for this chat: in the new-chat dialog, or from the account’s settings.'
    },
    {
      key: 'activity-hooks',
      class: 'runtime',
      value: 'hooks in C:\\Users\\dev\\Luna\\hooks\\demo-1.json',
      why: 'How Luna learns that the chat works, waits or asks: each event is posted to Luna’s local listener, and they print nothing.'
    },
    {
      key: 'core-read-deny',
      class: 'boundary',
      value: 'permissions.deny Read(//c/Users/dev/Luna/core/**) in C:\\Users\\dev\\Luna\\hooks\\demo-1.json',
      why: 'Keeps the session’s file tools out of Luna’s core folder, whose token reaches every session (threat model F3).'
    },
    {
      key: 'luna-tools',
      class: 'userAction',
      value:
        '--mcp-config C:\\Users\\dev\\Luna\\mcp\\demo-1.json, --allowedTools mcp__luna; its token in LUNA_MCP_TOKEN, not shown',
      why: 'The Luna tools box (or the agent that spawned it): Luna’s MCP server, its tools allowed without a prompt.'
    },
    {
      key: 'notes-dir',
      class: 'runtime',
      value: '--add-dir C:\\Users\\dev\\Luna\\agents',
      why: 'Long prompts, messages and helpers’ answers from Luna arrive as files there, readable without asking.'
    }
  ],
  account: [
    {
      key: 'trust',
      class: 'userAction',
      value: 'C:\\src\\luna',
      why: 'You ticked Trust this folder in the new-chat dialog: Luna wrote the same mark the CLI’s own trust prompt writes.'
    }
  ],
  incomplete: null,
  handoff: {
    instructions: {
      name: 'session-handoff',
      state: 'luna',
      version: '0.12.0',
      sha256: '4f2a9c81d03be7a65c1f0e2d9b8a7c6e5d4f3a2b1c0d9e8f7a6b5c4d3e2f1a0b',
      path: 'C:\\Users\\dev\\Luna\\library\\system\\session-handoff\\SKILL.md',
      problem: null
    },
    sameNamed: []
  }
};

// Settings → Links: the browsers a Windows machine might have, two Chrome
// profiles among them, and one account sent to the second.
export const BROWSERS: BrowsersDto = {
  system: String.raw`"C:\Program Files\Mozilla Firefox\firefox.exe" -osint -url "%1"`,
  found: [
    {
      id: 'edge',
      name: 'Microsoft Edge',
      command: String.raw`"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe" %1`
    },
    {
      id: 'chrome:Default',
      name: 'Google Chrome — Work',
      command: String.raw`"C:\Program Files\Google\Chrome\Application\chrome.exe" --profile-directory="Default" %1`
    },
    {
      id: 'chrome:Profile 1',
      name: 'Google Chrome — Personal',
      command: String.raw`"C:\Program Files\Google\Chrome\Application\chrome.exe" --profile-directory="Profile 1" %1`
    }
  ],
  settings: { custom: [], default: 'chrome:Default', accounts: { 'claude/personal': 'chrome:Profile 1' }, folders: {} }
};
