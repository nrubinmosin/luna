import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Account } from '../../shared/types';
import { useAccounts } from '../accounts/accounts.store';
import { allChats, useChats } from '../chats/chats.store';
import { NewChatDialog } from './NewChatDialog';
import { useNewChat } from './newchat.store';

// What the dialog asks the core as it opens: no settings files say anything,
// every folder is trusted, and Codex has no models listed yet.
vi.mock('../../ipc/defaults', async importOriginal => ({
  ...(await importOriginal<typeof import('../../ipc/defaults')>()),
  claudeDefaults: vi.fn(() => Promise.resolve({ model: null, effort: null, permissionMode: null })),
  codexDefaults: vi.fn(() => Promise.resolve({ model: null, effort: null, approval: null, sandbox: null })),
  codexModels: vi.fn(() => Promise.resolve([]))
}));
vi.mock('../../ipc/trust', async importOriginal => ({
  ...(await importOriginal<typeof import('../../ipc/trust')>()),
  claudeFolderTrusted: vi.fn(() => Promise.resolve(true)),
  codexFolderTrusted: vi.fn(() => Promise.resolve(true)),
  codexHooksStart: vi.fn(() => Promise.resolve({ default: false, kept: null }))
}));

// xterm cannot run in jsdom.
vi.mock('../panes/terminals', () => ({
  startSession: vi.fn(),
  focusTerminal: vi.fn(),
  forget: vi.fn(),
  forgetAll: vi.fn()
}));

const account = (provider: Account['provider'], name: string): Account => ({
  provider,
  name,
  path: `/accounts/${provider}/${name}`,
  plan: 'max',
  email: null,
  signedIn: true,
  haveUsage: false,
  limits: [],
  usageAge: null,
  fetchedAt: null,
  sync: 'ready',
  refreshError: null
});

/** The account box's face: the account the chat will be made on. */
const accountShown = () => screen.getByTitle(/ — (Claude Code|Codex)$/);

beforeEach(() => {
  useAccounts.setState({ accounts: [account('claude', 'work'), account('codex', 'gpt')], loaded: true });
  useChats.getState().rememberFolder('/repo');
  useNewChat.getState().openDialog('/repo');
});

describe('NewChatDialog: the account it starts on', () => {
  it('is the account the last chat was made on', () => {
    useNewChat.getState().remember('/repo', { provider: 'codex', name: 'gpt' });
    render(<NewChatDialog />);
    expect(accountShown()).toHaveTextContent('gpt');
  });

  it('is the first account when the last one is gone', () => {
    useNewChat.getState().remember('/repo', { provider: 'claude', name: 'deleted' });
    render(<NewChatDialog />);
    expect(accountShown()).toHaveTextContent('work');
  });

  it('tells a same-named account of the other provider apart', () => {
    useAccounts.setState({ accounts: [account('claude', 'main'), account('codex', 'main')] });
    useNewChat.getState().remember('/repo', { provider: 'codex', name: 'main' });
    render(<NewChatDialog />);
    expect(accountShown()).toHaveAttribute('title', 'main — Codex');
  });
});

describe('NewChatDialog: creating', () => {
  it('makes the chat on the account and folder shown, with the settings files’ defaults, and closes', async () => {
    const user = userEvent.setup();
    render(<NewChatDialog />);
    await user.click(screen.getByRole('button', { name: 'Create' }));
    const [chat] = allChats(useChats.getState().folders);
    expect(chat).toMatchObject({ provider: 'claude', account: 'work', model: 'Opus', effort: 'high', perm: 'Bypass' });
    expect(useChats.getState().folderOf(chat!.id)?.path).toBe('/repo');
    expect(useNewChat.getState().open).toBe(false);
    expect(useNewChat.getState().lastAccount).toEqual({ provider: 'claude', name: 'work' });
  });

  it('cannot create with no account to make it on', () => {
    useAccounts.setState({ accounts: [] });
    render(<NewChatDialog />);
    expect(screen.getByRole('button', { name: 'Create' })).toBeDisabled();
  });
});
