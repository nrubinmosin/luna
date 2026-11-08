import { act, render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { ACCOUNTS, FOLDERS } from '../../dev/demo/fixtures';
import type { BrowserSettingsDto } from '../../ipc/browsers';
import { useAccounts } from '../accounts/accounts.store';
import { useChats } from '../chats/chats.store';
import { SettingsDialog } from './SettingsDialog';
import { useSettings, type SettingsPage } from './settings.store';

// The browsers as a machine with none but the system's would report them, and
// a save that hands back what it was given, as the Rust side does.
vi.mock('../../ipc/browsers', async importOriginal => ({
  ...(await importOriginal<typeof import('../../ipc/browsers')>()),
  browsers: () =>
    Promise.resolve({ system: null, found: [], settings: { custom: [], default: null, accounts: {}, folders: {} } }),
  setBrowsers: (settings: BrowserSettingsDto) => Promise.resolve({ system: null, found: [], settings })
}));

function open(page: SettingsPage = 'general') {
  act(() => useSettings.getState().setOpen(true, page));
  render(
    <div data-app>
      <SettingsDialog />
    </div>
  );
  return userEvent.setup();
}

// A note kept in view, not the same words folded into a group's explanation.
const NOTE = '.set-note span';
const tab = (name: string) => screen.getByRole('tab', { name });

describe('SettingsDialog', () => {
  it('walks its pages with the arrows, Home and End, and tabs on into the page', async () => {
    const user = open();
    expect(tab('General')).toHaveFocus();
    expect(tab('General')).toHaveAttribute('aria-selected', 'true');
    expect(screen.getByRole('heading', { name: 'General', level: 2 })).toBeInTheDocument();

    await user.keyboard('{ArrowDown}');
    expect(tab('Chats')).toHaveFocus();
    expect(screen.getByRole('heading', { name: 'Chats', level: 2 })).toBeInTheDocument();
    await user.keyboard('{End}');
    expect(tab('Updates')).toHaveFocus();
    await user.keyboard('{ArrowDown}');
    expect(tab('General')).toHaveFocus();
    await user.keyboard('{ArrowUp}');
    expect(tab('Updates')).toHaveFocus();
    await user.keyboard('{Home}');
    expect(useSettings.getState().page).toBe('general');

    // One tab stop for the whole list: the next Tab is in the page.
    await user.tab();
    expect(screen.getByRole('tabpanel')).toContainElement(document.activeElement as HTMLElement);
    expect(screen.getByRole('checkbox', { name: 'Auto-tile the board' })).toHaveFocus();
  });

  it('keeps what a change costs in view, and folds the rest under a named summary', () => {
    open('agents');
    const helpers = screen.getByRole('checkbox', { name: 'Allow helpers more permissions than their parent chat' });
    expect(helpers).not.toBeChecked();
    expect(
      screen.getByText(/may spawn helpers that bypass permissions or run with full access/, { selector: NOTE })
    ).toBeVisible();
    const summary = screen.getByText('How helper permissions work');
    expect(summary.closest('details')).not.toHaveAttribute('open');

    act(() => useSettings.getState().setPage('chats'));
    expect(screen.getByRole('checkbox', { name: 'Delete worktrees by default' })).toBeInTheDocument();
    expect(screen.getByText('Uncommitted work in a deleted worktree is lost.', { selector: NOTE })).toBeVisible();

    act(() => useSettings.getState().setPage('general'));
    expect(screen.getByText(/move the account folders yourself/, { selector: NOTE })).toBeVisible();
  });

  it('says so when a list is empty', async () => {
    open('agents');
    expect(await screen.findByText('No trusted folders. Codex asks in every project.')).toBeInTheDocument();
    act(() => useSettings.getState().setPage('links'));
    expect(await screen.findByText('No exceptions. Every chat uses the default browser.')).toBeInTheDocument();
    expect(screen.getByText('Only the system default was found.')).toBeInTheDocument();
  });

  it('adds an exception from its menu, and Escape shuts the menu before Settings', async () => {
    act(() => {
      useAccounts.setState({ accounts: ACCOUNTS });
      useChats.setState({ folders: FOLDERS });
    });
    const user = open('links');
    await user.click(await screen.findByRole('button', { name: 'Add an exception…' }));
    const menu = screen.getByRole('menu', { name: 'Add an exception' });
    expect(within(menu).getByRole('menuitem', { name: 'Claude Code · personal' })).toHaveFocus();

    await user.keyboard('{Escape}');
    expect(screen.queryByRole('menu')).not.toBeInTheDocument();
    expect(useSettings.getState().open).toBe(true);

    await user.click(screen.getByRole('button', { name: 'Add an exception…' }));
    await user.click(screen.getByRole('menuitem', { name: 'C:\\src\\notes' }));
    const list = screen.getByRole('list', { name: 'Exceptions' });
    expect(await within(list).findByText('C:\\src\\notes')).toBeInTheDocument();
    expect(within(list).getByRole('button', { name: 'Remove the exception for C:\\src\\notes' })).toBeInTheDocument();

    await user.keyboard('{Escape}');
    expect(useSettings.getState().open).toBe(false);
  });
});
