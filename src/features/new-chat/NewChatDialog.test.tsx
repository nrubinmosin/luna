import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import * as defaults from '../../ipc/defaults';
import * as trust from '../../ipc/trust';
import type { ClaudeDefaultsDto, CodexDefaultsDto } from '../../ipc/defaults';
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
  claudeTrustFolder: vi.fn(() => Promise.resolve()),
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

const mocked = vi.mocked(defaults);
const STOCK_CLAUDE: ClaudeDefaultsDto = { model: null, effort: null, permissionMode: null };
const STOCK_CODEX: CodexDefaultsDto = { model: null, effort: null, approval: null, sandbox: null };
/** Lets the answers already given land in the dialog. */
const settle = () => act(() => new Promise<void>(resolve => setTimeout(resolve, 0)));
/** What an account's settings file says, as the core reports it. */
const fromAccount = (value: string) => ({ value, source: 'account' as const });

beforeEach(() => {
  // What a test told the settings to say goes back to nothing said.
  vi.mocked(trust.claudeFolderTrusted).mockResolvedValue(true);
  vi.mocked(trust.claudeTrustFolder).mockClear();
  mocked.claudeDefaults.mockImplementation(() => Promise.resolve(STOCK_CLAUDE));
  mocked.codexDefaults.mockImplementation(() => Promise.resolve(STOCK_CODEX));
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

  it('takes Claude Code’s model, effort and permission mode from the account’s settings', async () => {
    mocked.claudeDefaults.mockResolvedValue({
      model: fromAccount('claude-sonnet-5'),
      effort: fromAccount('low'),
      permissionMode: fromAccount('plan')
    });
    const user = userEvent.setup();
    render(<NewChatDialog />);
    await waitFor(() => expect(mocked.claudeDefaults).toHaveBeenCalledWith('/accounts/claude/work', '/repo'));
    await settle();
    await user.click(screen.getByRole('button', { name: 'Create' }));
    const [chat] = allChats(useChats.getState().folders);
    expect(chat).toMatchObject({ provider: 'claude', model: 'Sonnet', effort: 'low', perm: 'Plan only' });
  });

  it('takes Codex’s model, effort, approval and sandbox from the account’s config', async () => {
    mocked.codexDefaults.mockResolvedValue({
      model: fromAccount('gpt-5.4-codex'),
      effort: fromAccount('low'),
      approval: fromAccount('on-request'),
      sandbox: fromAccount('workspace-write')
    });
    useNewChat.getState().remember('/repo', { provider: 'codex', name: 'gpt' });
    const user = userEvent.setup();
    render(<NewChatDialog />);
    await waitFor(() => expect(mocked.codexDefaults).toHaveBeenCalledWith('/accounts/codex/gpt', '/repo'));
    await settle();
    await user.click(screen.getByRole('button', { name: 'Create' }));
    const [chat] = allChats(useChats.getState().folders);
    expect(chat).toMatchObject({
      provider: 'codex',
      account: 'gpt',
      model: 'gpt-5.4-codex',
      effort: 'low',
      approval: 'on-request',
      sandbox: 'workspace-write'
    });
  });

  it('keeps the settings of the account picked last, though the first one’s answer comes in after', async () => {
    useAccounts.setState({ accounts: [account('claude', 'work'), account('claude', 'home')] });
    useNewChat.getState().remember('/repo', { provider: 'claude', name: 'work' });
    let answerWork: (dto: ClaudeDefaultsDto) => void = () => {};
    mocked.claudeDefaults.mockImplementation(path =>
      path === '/accounts/claude/work'
        ? new Promise(resolve => (answerWork = resolve))
        : Promise.resolve({
            model: fromAccount('haiku'),
            effort: fromAccount('medium'),
            permissionMode: fromAccount('default')
          })
    );
    const user = userEvent.setup();
    render(<NewChatDialog />);
    await user.click(accountShown());
    await user.click(screen.getByRole('option', { name: /home/ }));
    await waitFor(() => expect(mocked.claudeDefaults).toHaveBeenCalledWith('/accounts/claude/home', '/repo'));
    // The first account's files answer late, and must not land on the second.
    answerWork({ model: fromAccount('opus'), effort: fromAccount('max'), permissionMode: fromAccount('acceptEdits') });
    await settle();
    await user.click(screen.getByRole('button', { name: 'Create' }));
    const [chat] = allChats(useChats.getState().folders);
    expect(chat).toMatchObject({ account: 'home', model: 'Haiku', effort: 'medium', perm: 'Ask' });
  });

  it('cannot create with no account to make it on', () => {
    useAccounts.setState({ accounts: [] });
    render(<NewChatDialog />);
    expect(screen.getByRole('button', { name: 'Create' })).toBeDisabled();
  });
});

describe('NewChatDialog: what Luna adds only when asked', () => {
  it('writes no trust unless the box is ticked, and then the CLI’s own mark', async () => {
    vi.mocked(trust.claudeFolderTrusted).mockResolvedValue(false);
    const write = vi.mocked(trust.claudeTrustFolder).mockResolvedValue();
    const user = userEvent.setup();
    const first = render(<NewChatDialog />);
    await settle();
    expect(screen.getByRole('checkbox', { name: 'Trust this folder in “work”' })).not.toBeChecked();
    await user.click(screen.getByRole('button', { name: 'Create' }));
    expect(write).not.toHaveBeenCalled();
    first.unmount();

    useNewChat.getState().openDialog('/repo');
    render(<NewChatDialog />);
    await settle();
    await user.click(screen.getByRole('checkbox', { name: 'Trust this folder in “work”' }));
    await user.click(screen.getByRole('button', { name: 'Create' }));
    expect(write).toHaveBeenCalledWith('/accounts/claude/work', '/repo');
  });

  it('binds a tick to its folder: another one picked is asked about afresh, and pending until answered', async () => {
    useChats.getState().rememberFolder('/other');
    let answerOther: (trusted: boolean) => void = () => {};
    vi.mocked(trust.claudeFolderTrusted).mockImplementation((_account, folder) =>
      folder === '/other' ? new Promise(resolve => (answerOther = resolve)) : Promise.resolve(false)
    );
    const write = vi.mocked(trust.claudeTrustFolder);
    const user = userEvent.setup();
    render(<NewChatDialog />);
    await settle();
    const worktree = screen.getByRole('checkbox', { name: 'Git worktree' });
    if ((worktree as HTMLInputElement).checked) await user.click(worktree);
    await user.click(screen.getByRole('checkbox', { name: 'Trust this folder in “work”' }));
    // Another folder, its answer still on the way: no Create, and no box.
    await user.click(screen.getByTitle('/other'));
    expect(screen.getByRole('button', { name: 'Create' })).toBeDisabled();
    expect(screen.queryByRole('checkbox', { name: /Trust this folder/ })).toBeNull();
    await user.keyboard('{Enter}');
    expect(write).not.toHaveBeenCalled();
    // Answered untrusted: asked afresh, unticked; created so, nothing written.
    act(() => answerOther(false));
    await settle();
    expect(screen.getByRole('checkbox', { name: 'Trust this folder in “work”' })).not.toBeChecked();
    // The folder picked starts its boxes afresh, the worktree's among them.
    const again = screen.getByRole('checkbox', { name: 'Git worktree' });
    if ((again as HTMLInputElement).checked) await user.click(again);
    await user.click(screen.getByRole('button', { name: 'Create' }));
    expect(write).not.toHaveBeenCalled();
    const [chat] = allChats(useChats.getState().folders);
    expect(useChats.getState().folderOf(chat!.id)?.path).toBe('/other');
  });

  it('will not make a Claude Code worktree in a folder it cannot be asked about', async () => {
    vi.mocked(trust.claudeFolderTrusted).mockResolvedValue(false);
    const user = userEvent.setup();
    render(<NewChatDialog />);
    await settle();
    const worktree = screen.getByRole('checkbox', { name: 'Git worktree' });
    if (!(worktree as HTMLInputElement).checked) await user.click(worktree);
    expect(screen.getByRole('button', { name: 'Create' })).toBeDisabled();
    await user.click(worktree);
    expect(screen.getByRole('button', { name: 'Create' })).toBeEnabled();
    await user.click(worktree);
    await user.click(screen.getByRole('checkbox', { name: 'Trust this folder in “work”' }));
    expect(screen.getByRole('button', { name: 'Create' })).toBeEnabled();
  });

  it('starts a chat without the wait guard unless it is ticked', async () => {
    const user = userEvent.setup();
    render(<NewChatDialog />);
    const box = screen.getByRole('checkbox', { name: 'Wait guard' });
    expect(box).not.toBeChecked();
    await user.click(box);
    await user.click(screen.getByRole('button', { name: 'Create' }));
    const [chat] = allChats(useChats.getState().folders);
    expect(chat).toMatchObject({ guard: true });
  });

  it('offers auto-handoff only with Luna tools, off unless ticked, with its threshold', async () => {
    const user = userEvent.setup();
    render(<NewChatDialog />);
    const tools = screen.getByRole('checkbox', { name: 'Luna tools' });
    if ((tools as HTMLInputElement).checked) await user.click(tools);
    const box = screen.getByRole('checkbox', { name: 'Auto-handoff at' });
    expect(box).toBeDisabled();
    expect(box).not.toBeChecked();
    await user.click(tools);
    expect(box).toBeEnabled();
    await user.click(box);
    const at = screen.getByRole('spinbutton', { name: /Auto-handoff threshold/ });
    await user.clear(at);
    await user.type(at, '60');
    await user.click(screen.getByRole('button', { name: 'Create' }));
    const [chat] = allChats(useChats.getState().folders);
    expect(chat).toMatchObject({ tools: true, autoHandoff: true, handoffAt: 60 });
  });

  it('offers Codex the wait guard only with its hooks unasked', async () => {
    useNewChat.getState().remember('/repo', { provider: 'codex', name: 'gpt' });
    const user = userEvent.setup();
    render(<NewChatDialog />);
    await settle();
    expect(screen.getByRole('checkbox', { name: 'Wait guard' })).toBeDisabled();
    await user.click(screen.getByRole('checkbox', { name: 'Hooks unasked' }));
    expect(screen.getByRole('checkbox', { name: 'Wait guard' })).toBeEnabled();
  });
});
