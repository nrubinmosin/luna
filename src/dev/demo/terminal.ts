// The panes' invented sessions, painted in ANSI the way each CLI draws its own.
import type { Terminal as XTerm } from '@xterm/xterm';

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
    '  • Chats keep their own color across panes and the sidebar\r\n' +
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
