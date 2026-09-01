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
import type { Account, Chat, Folder } from '../shared/types';
import type { AccountsRootInfo, CliStatusDto } from '../ipc/commands';
import { useAccounts } from '../features/accounts/accounts.store';
import { useChats } from '../features/chats/chats.store';
import { useNewChat } from '../features/new-chat/newchat.store';
import { usePanes, type Group, type Layout, type Slots } from '../features/panes/panes.store';
import { useUpdates } from '../features/updates/updates.store';

/** `?demo`, `?demo=dark`, `?demo=newchat`, `?demo=settings`. */
export type Scene = 'main' | 'dark' | 'newchat' | 'settings';

const hoursFromNow = (h: number) => new Date(Date.now() + h * 3600_000).toISOString();

const chat = (c: Partial<Chat> & Pick<Chat, 'id' | 'name'>): Chat => ({
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

const CHATS: Chat[] = [
  chat({
    id: 'demo-1',
    name: 'Updater offered a build its own version',
    status: 'working',
    model: 'Opus',
    effort: 'max',
    context: 0.34,
    color: 'sky'
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
  chat({
    id: 'demo-4',
    name: 'Draft the 0.3 release notes',
    status: 'working',
    model: 'Haiku',
    effort: 'low',
    perm: 'Plan only',
    context: 0.08,
    account: 'work',
    worktree: false,
    color: 'magenta'
  })
];

const FOLDERS: Folder[] = [
  { id: 'f-1', path: 'C:\\src\\luna', open: true, chats: CHATS.slice(0, 3) },
  { id: 'f-2', path: 'C:\\src\\notes', open: true, chats: CHATS.slice(3) }
];

const account = (a: Partial<Account> & Pick<Account, 'name'>): Account => ({
  path: `C:\\src\\claude-accounts\\${a.name}`,
  plan: 'Max 20×',
  email: null,
  signedIn: true,
  haveUsage: true,
  limits: { h5: 0.12, week: 0.41, fable: 0.27 },
  resets: { h5: '3h 40m', week: '4d', fable: '4d' },
  weekResetAt: hoursFromNow(96),
  usageAge: 'just now',
  fetchedAt: Date.now(),
  sync: 'ready',
  ...a
});

const ACCOUNTS: Account[] = [
  account({ name: 'personal' }),
  account({
    name: 'work',
    plan: 'Max 5×',
    limits: { h5: 0.63, week: 0.78, fable: 0.44 },
    resets: { h5: '52m', week: '2d', fable: '2d' }
  }),
  account({
    name: 'spare',
    plan: '—',
    signedIn: false,
    haveUsage: false,
    limits: { h5: 0, week: 0, fable: 0 },
    resets: { h5: '—', week: '—', fable: '—' },
    weekResetAt: null,
    usageAge: null,
    fetchedAt: null,
    sync: 'ready'
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
    header('Haiku 4.5', 'low', 'C:\\src\\notes') +
    '\r\n' +
    user('draft release notes for 0.3 from the log since 0.2.16\r\n') +
    '\r\n' +
    bullet('Reading 41 commits…') +
    '\r\n' +
    `  ${bold('Luna 0.3')}\r\n` +
    '  • Chats keep their own colour across panes and the sidebar\r\n' +
    '  • Window groups I–IV, each with its own four boards\r\n' +
    '  • The app ships and updates its own copy of the CLI\r\n' +
    '\r\n' +
    `${c(33, '✻')} Drafting… ${dim('(8s · ↓ 1.2k tokens)')}\r\n`
};

/** Paints one pane's invented session. Called from Terminal only in dev. */
export function paint(term: XTerm, chatId: string) {
  const screen = SCREENS[chatId];
  if (screen) term.write(screen);
}

// ------------------------------------------------------------- ipc stand-in --

const CLI: CliStatusDto = {
  phase: 'idle',
  version: '2.1.252',
  path: 'C:\\src\\luna\\claude-cli\\versions\\2.1.252\\claude.exe',
  latest: '2.1.252',
  got: 0,
  total: null,
  error: null,
  checkedAtMs: Date.now() - 42 * 60_000
};

const ROOT: AccountsRootInfo = { path: 'C:\\src\\claude-accounts', isDefault: true };

let seeded = false;

/**
 * Answers the few commands whose emptiness would show. Without this the
 * settings dialog draws no CLI version and no accounts folder in a browser,
 * and a screenshot of it is a picture of blanks. Everything else keeps the
 * caller's own fallback, and outside `?demo` this does nothing at all.
 */
export function answer<T>(cmd: string, fallback: T): T {
  if (!seeded) return fallback;
  if (cmd === 'cli_status') return CLI as unknown as T;
  if (cmd === 'get_accounts_root') return ROOT as unknown as T;
  return fallback;
}

// ------------------------------------------------------------------ seed --

export function seedDemo(scene: Scene) {
  seeded = true;
  localStorage.setItem('luna.theme', scene === 'dark' ? 'dark' : 'light');
  // The version readout, which in a browser has no app to ask.
  useUpdates.setState({ current: '0.2.17', checkedAt: Date.now() - 12 * 60_000 });

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
    groups: [filled(scene === 'dark' ? 2 : 4), parked(), parked(), parked()]
  });

  if (scene === 'newchat') useNewChat.getState().openDialog('C:\\src\\luna');
}

/** `?demo` / `?demo=dark` — which fixture the URL is asking for, if any. */
export function sceneFromUrl(search: string): Scene | null {
  const value = new URLSearchParams(search).get('demo');
  if (value === null) return null;
  return value === 'dark' || value === 'newchat' || value === 'settings' ? value : 'main';
}
