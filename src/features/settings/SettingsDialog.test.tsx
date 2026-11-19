import { act, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';
import { useNewChat } from '../new-chat/newchat.store';
import { UpdateConfirm } from '../updates/UpdateConfirm';
import { useUpdates } from '../updates/updates.store';
import { SettingsDialog } from './SettingsDialog';
import { useSettings, type SettingsPage } from './settings.store';

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

    act(() => useSettings.getState().setPage('general'));
    expect(screen.getByText(/move the account folders yourself/, { selector: NOTE })).toBeVisible();
  });

  it('says so when a list is empty', async () => {
    open('agents');
    expect(await screen.findByText('No trusted folders. Codex asks in every project.')).toBeInTheDocument();
  });

  it('is modal: the app behind is inert, Tab goes round, and the focus goes back to the opener', async () => {
    render(
      <div data-app>
        <div data-testid="app">
          <button>Gear</button>
        </div>
        <SettingsDialog />
      </div>
    );
    const user = userEvent.setup();
    const gear = screen.getByRole('button', { name: 'Gear' });
    gear.focus();
    act(() => useSettings.getState().setOpen(true));

    expect(screen.getByTestId('app')).toHaveAttribute('inert');
    expect(tab('General')).toHaveFocus();
    const close = screen.getByRole('button', { name: 'Close' });
    await user.tab({ shift: true });
    expect(close).toHaveFocus();
    await user.tab({ shift: true });
    const last = document.activeElement as HTMLElement;
    expect(screen.getByRole('dialog')).toContainElement(last);
    expect(last).not.toBe(close);
    await user.tab();
    expect(close).toHaveFocus();

    await user.keyboard('{Escape}');
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(screen.getByTestId('app')).not.toHaveAttribute('inert');
    expect(gear).toHaveFocus();
  });

  it('is suspended under the update confirm it raises, and gets the focus back after', async () => {
    act(() => useUpdates.setState({ phase: 'available', current: '0.10.1', next: '0.11.0' }));
    render(
      <div data-app>
        <SettingsDialog />
        <UpdateConfirm />
      </div>
    );
    const user = userEvent.setup();
    act(() => useSettings.getState().setOpen(true, 'updates'));
    const update = screen.getByRole('button', { name: 'Update…' });
    await user.click(update);

    const confirm = screen.getByRole('dialog', { name: 'Update to 0.11.0?' });
    expect(confirm).toHaveFocus();
    const settings = document.querySelector('.settings')!;
    expect(settings.closest('[inert]')).not.toBeNull();
    // Tab walks the confirm's own controls and never lands in Settings.
    for (let i = 0; i < 6; i++) {
      await user.tab();
      expect(confirm).toContainElement(document.activeElement as HTMLElement);
    }

    await user.keyboard('{Escape}');
    expect(screen.queryByRole('dialog', { name: 'Update to 0.11.0?' })).not.toBeInTheDocument();
    expect(useSettings.getState().open).toBe(true);
    expect(settings.closest('[inert]')).toBeNull();
    expect(update).toHaveFocus();
  });

  it('keeps a draft on one page while another is looked at', async () => {
    const user = open('general');
    const marker = await screen.findByRole('textbox', { name: 'Waiting marker text' });
    await user.clear(marker);
    await user.type(marker, 'STATUS: WAITING');

    await user.click(tab('Chats'));
    await user.click(tab('General'));
    expect(screen.getByRole('textbox', { name: 'Waiting marker text' })).toHaveValue('STATUS: WAITING');
  });

  it('sets where the new-chat boxes start, the wait guard off unless asked', async () => {
    const user = open('chats');
    const guard = screen.getByRole('checkbox', { name: 'Wait guard' });
    expect(guard).not.toBeChecked();
    await user.click(guard);
    expect(useNewChat.getState().guard).toBe(true);
    expect(guard).toBeChecked();
  });
});
