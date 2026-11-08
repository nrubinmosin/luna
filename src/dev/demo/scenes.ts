/**
 * The fixture behind the README screenshots.
 *
 * Shooting the real app means shooting whatever is open in it — someone's
 * folders, chat titles, account emails and half a client's codebase. This
 * stands the same UI up on invented data instead: `pnpm dev` with `?demo` in
 * the URL, no Tauri and no sessions, so the shots can be retaken from a clean
 * slate whenever the UI moves on. See `scripts/screenshots.ps1`.
 *
 * Dev-only by construction: every entry point is behind `import.meta.env.DEV`,
 * so none of this is in a release build.
 */
import { useLibrary } from '../../features/library/library.store';
import { useRecommendations } from '../../features/settings/recommendations.store';
import { useBackground } from '../../features/agents/background.store';
import { useAccounts } from '../../features/accounts/accounts.store';
import { useAccountsWindow } from '../../features/accounts/AccountsPanel';
import { useChats } from '../../features/chats/chats.store';
import { useDrafts } from '../../features/chats/drafts.store';
import { useWorktrees } from '../../features/chats/worktrees.store';
import { useNewChat } from '../../features/new-chat/newchat.store';
import { usePanes } from '../../features/panes/panes.store';
import { useUpdates } from '../../features/updates/updates.store';
import { useShell } from '../../app/shell.store';
import { usePower } from '../../features/power/power.store';
import { SETTINGS_PAGES, useSettings, type SettingsPage } from '../../features/settings/settings.store';
import { FOLDERS, ACCOUNTS, filled, parked, BACKGROUND } from './fixtures';
import { demoState } from './ipc';

/** A Settings page's scene: `settings` is its first page, `settings-links` and
 *  the like the others, each with a `-dark` twin. */
type SettingsScene = 'settings' | `settings-${Exclude<SettingsPage, 'general'>}`;
const SETTINGS_SCENES = SETTINGS_PAGES.map((p): SettingsScene => (p === 'general' ? 'settings' : `settings-${p}`));

/** `?demo`, `?demo=dark`, `?demo=peek`, `?demo=newchat`, `?demo=codex` (the same on a Codex
 *  account), `?demo=settings` / `settings-dark` and one per page (`settings-chats`, …), `?demo=accounts`,
 *  `?demo=recommended`, `?demo=editor`, `?demo=worktrees`,
 *  `?demo=checks` / `checks-dark` (the Recommendations window), `checks-bar` / `checks-bar-dark` (their chip),
 *  `?demo=library`, `?demo=power`, `?demo=applied` / `applied-dark` (a chat's What Luna applied),
 *  `?demo=helper` / `helper-dark` (a helper watched read-only),
 *  `?demo=delete-helpers` (deleting a chat whose helpers are at work). */
export type Scene =
  | 'main'
  | 'dark'
  | 'peek'
  | 'newchat'
  | 'codex'
  | SettingsScene
  | `${SettingsScene}-dark`
  | 'accounts'
  | 'recommended'
  | 'editor'
  | 'worktrees'
  | 'library'
  | 'power'
  | 'checks'
  | 'checks-dark'
  | 'checks-bar'
  | 'checks-bar-dark'
  | 'applied'
  | 'applied-dark'
  | 'helper'
  | 'helper-dark'
  | 'delete-helpers';
const SCENES: Scene[] = [
  'dark',
  'peek',
  'newchat',
  'codex',
  ...SETTINGS_SCENES.flatMap(s => [s, `${s}-dark` as const]),
  'accounts',
  'recommended',
  'editor',
  'worktrees',
  'library',
  'power',
  'checks',
  'checks-dark',
  'checks-bar',
  'checks-bar-dark',
  'applied',
  'applied-dark',
  'helper',
  'helper-dark',
  'delete-helpers'
];

export function seedDemo(scene: Scene) {
  demoState.seeded = true;
  demoState.showsChecks = scene.startsWith('checks');
  useBackground.setState({ helpers: BACKGROUND });
  // The shell store has read the theme by now; tell it, not localStorage.
  useShell.setState({ theme: scene.endsWith('dark') ? 'dark' : 'light', pinned: true });
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

  const settings = SETTINGS_SCENES.findIndex(s => s === scene || `${s}-dark` === scene);
  if (settings >= 0) useSettings.getState().setOpen(true, SETTINGS_PAGES[settings]);
  if (scene === 'codex') useNewChat.setState({ lastAccount: { provider: 'codex', name: 'personal' } });
  if (scene === 'newchat' || scene === 'codex') useNewChat.getState().openDialog('C:\\src\\luna');
  if (scene === 'accounts' || scene === 'recommended' || scene === 'editor') useAccountsWindow.getState().setOpen(true);
  if (scene === 'worktrees') useWorktrees.getState().setOpen(true);
  if (scene === 'checks' || scene === 'checks-dark') {
    useRecommendations.getState().setOpen(true);
    // The confirm a risky fix asks for, opened the way a click would.
    setTimeout(() => document.querySelector<HTMLElement>('[data-confirms]')?.click(), 600);
  }
  if (scene === 'power') {
    demoState.power = { ...demoState.power, armed: { action: 'restart', quietS: 120, armedAtMs: 0, drain: true } };
    usePower.setState({ state: demoState.power, open: true });
  }
  if (scene === 'library') {
    useLibrary.getState().setOpen(true);
    // The changed copy's differences, opened the way a click would.
    setTimeout(
      () =>
        [...document.querySelectorAll<HTMLButtonElement>('button')].find(b => b.textContent === 'Differences')?.click(),
      600
    );
  }
  // A chat's panel, opened from its title bar the way a click would.
  if (scene.startsWith('applied')) {
    setTimeout(() => document.querySelector<HTMLElement>('button[aria-label="What Luna applied"]')?.click(), 300);
  }
  // A helper of the first chat, watched: its chip, then Watch, as clicks would.
  if (scene === 'helper' || scene === 'helper-dark') {
    setTimeout(() => {
      document.querySelector<HTMLElement>('.chat-row[data-active="yes"] .kids')?.click();
      setTimeout(
        () =>
          [...document.querySelectorAll<HTMLButtonElement>('.popover button')]
            .find(b => b.textContent === 'Watch')
            ?.click(),
        300
      );
    }, 300);
  }
  // The first chat's Delete, with its helpers listed and their boxes ticked.
  if (scene === 'delete-helpers') {
    setTimeout(() => {
      document.querySelector<HTMLElement>('.chat-row[data-active="yes"] [title^="Delete chat"]')?.click();
      setTimeout(() => document.querySelector<HTMLElement>('label[for^="drop-helpers-"]')?.click(), 400);
    }, 300);
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
