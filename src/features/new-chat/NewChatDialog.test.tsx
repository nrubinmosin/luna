import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import * as defaults from '../../ipc/defaults';
import * as sessions from '../../ipc/sessions';
import * as trust from '../../ipc/trust';
import type { ClaudeDefaultsDto, ClaudeModelDto, CodexDefaultsDto } from '../../ipc/defaults';
import type { Account } from '../../shared/types';
import { expectInDialogFoot } from '../../test/dialog';
import { useAccounts } from '../accounts/accounts.store';
import { allChats, useChats } from '../chats/chats.store';
import { quickChat } from './create';
import { NewChatDialog } from './NewChatDialog';
import { useNewChat } from './newchat.store';

// What the dialog asks the core as it opens: no settings files say anything,
// every folder is trusted, and neither CLI has a model list for the account
// yet (Claude's dialog then lists the four aliases).
vi.mock('../../ipc/defaults', async importOriginal => ({
  ...(await importOriginal<typeof import('../../ipc/defaults')>()),
  claudeDefaults: vi.fn(() => Promise.resolve({ model: null, effort: null, permissionMode: null })),
  claudeModels: vi.fn(() => Promise.resolve([])),
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

// Every folder is one a chat can start in, unless a test says otherwise.
vi.mock('../../ipc/sessions', async importOriginal => ({
  ...(await importOriginal<typeof import('../../ipc/sessions')>()),
  chatFolderCheck: vi.fn(() => Promise.resolve({ refusal: null, warning: null }))
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
  vi.mocked(sessions.chatFolderCheck).mockResolvedValue({ refusal: null, warning: null });
  vi.mocked(trust.claudeFolderTrusted).mockResolvedValue(true);
  vi.mocked(trust.claudeTrustFolder).mockClear();
  mocked.claudeDefaults.mockImplementation(() => Promise.resolve(STOCK_CLAUDE));
  mocked.claudeModels.mockImplementation(() => Promise.resolve([]));
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

  it('keeps Create and Cancel below the body, which is what scrolls on a short screen', () => {
    render(<NewChatDialog />);
    expectInDialogFoot(screen.getByRole('button', { name: 'Create' }));
    expectInDialogFoot(screen.getByRole('button', { name: 'Cancel' }));
  });

  it('takes Claude Code’s model, effort and permission mode from the account’s settings', async () => {
    mocked.claudeDefaults.mockResolvedValue({
      model: fromAccount('sonnet'),
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

  it('says why a folder will not do, and creates nothing there', async () => {
    const why = '/repo does not exist (moved or deleted?): pick another folder';
    vi.mocked(sessions.chatFolderCheck).mockResolvedValue({ refusal: why, warning: null });
    const user = userEvent.setup();
    render(<NewChatDialog />);
    await settle();
    expect(screen.getByRole('alert')).toHaveTextContent(why);
    expect(screen.getByRole('button', { name: 'Create' })).toBeDisabled();
    await user.keyboard('{Enter}');
    expect(allChats(useChats.getState().folders)).toEqual([]);
  });

  it('asks again as Git worktree is flipped: a folder outside git takes no worktree', async () => {
    const why = '/repo is not in a git repository: untick Git worktree, or pick a folder in one';
    vi.mocked(sessions.chatFolderCheck).mockImplementation((_p, _a, _f, worktree) =>
      Promise.resolve({ refusal: worktree ? why : null, warning: null })
    );
    const user = userEvent.setup();
    render(<NewChatDialog />);
    const worktree = screen.getByRole('checkbox', { name: 'Git worktree' });
    if (!(worktree as HTMLInputElement).checked) await user.click(worktree);
    await settle();
    expect(screen.getByRole('alert')).toHaveTextContent(why);
    expect(screen.getByRole('button', { name: 'Create' })).toBeDisabled();
    await user.click(worktree);
    await settle();
    expect(screen.queryByRole('alert')).toBeNull();
    await user.click(screen.getByRole('button', { name: 'Create' }));
    const [chat] = allChats(useChats.getState().folders);
    expect(chat).toMatchObject({ worktree: false });
  });

  it('only warns where a source Luna cannot read may still make the worktree, and creates', async () => {
    const warning = '/repo is not in a git repository: Claude Code makes a worktree there only through a hook';
    vi.mocked(sessions.chatFolderCheck).mockResolvedValue({ refusal: null, warning });
    const user = userEvent.setup();
    render(<NewChatDialog />);
    const worktree = screen.getByRole('checkbox', { name: 'Git worktree' });
    if (!(worktree as HTMLInputElement).checked) await user.click(worktree);
    await settle();
    expect(screen.getByRole('status')).toHaveTextContent(warning);
    expect(screen.queryByRole('alert')).toBeNull();
    await user.click(screen.getByRole('button', { name: 'Create' }));
    const [chat] = allChats(useChats.getState().folders);
    expect(chat).toMatchObject({ worktree: true });
  });
});

describe('NewChatDialog: Claude models from the account’s own list', () => {
  const model = (
    slug: string,
    displayName: string,
    efforts: string[],
    defaultEffort: string | null,
    more = false
  ): ClaudeModelDto => ({ slug, modelId: null, displayName, defaultEffort, efforts, effortHints: {}, more });
  const LIST: ClaudeModelDto[] = [
    model('opus', 'Opus 5.5', ['low', 'medium', 'high', 'xhigh', 'max'], 'medium'),
    model('mythos', 'Mythos 1', ['low', 'high'], 'low'),
    model('claude-opus-4-7', 'Opus 4.7', ['medium', 'xhigh'], 'xhigh', true),
    model('claude-haiku-4-5-20251001', 'Haiku 4.5', [], null, true)
  ];
  // The effort row's buttons; the permission modes are radio inputs.
  const effortButtons = () =>
    screen
      .queryAllByRole('radio')
      .filter(r => r.tagName !== 'INPUT')
      .map(r => r.textContent);

  beforeEach(() => mocked.claudeModels.mockResolvedValue(LIST));

  it('lists main models first and the rest under More…', async () => {
    render(<NewChatDialog />);
    await settle();
    expect(mocked.claudeModels).toHaveBeenCalledWith('/accounts/claude/work');
    const select = screen.getByRole('combobox');
    const top = [...select.children].filter(c => c.tagName === 'OPTION').map(o => o.textContent);
    expect(top).toEqual(['Opus 5.5 · default', 'Mythos 1']);
    const more = screen.getByRole('group', { name: 'More…' });
    expect([...more.children].map(o => o.textContent)).toEqual(['Opus 4.7', 'Haiku 4.5']);
  });

  it('offers the chosen model’s own effort levels, and none for a model that takes none', async () => {
    const user = userEvent.setup();
    render(<NewChatDialog />);
    await settle();
    expect(effortButtons()).toEqual(['low', 'medium', 'high', 'xhigh', 'max', 'ultracode']);
    await user.selectOptions(screen.getByRole('combobox'), 'Mythos');
    // Stock `high` is a level Mythos takes, so it stays.
    expect(effortButtons()).toEqual(['low', 'high', 'ultracode']);
    await user.selectOptions(screen.getByRole('combobox'), 'claude-haiku-4-5-20251001');
    expect(effortButtons()).toEqual([]);
    expect(screen.getByText('Haiku 4.5 takes no effort level.')).toBeInTheDocument();
  });

  it('moves the effort to a model’s own default where the model does not take it, and runs it by full id', async () => {
    const user = userEvent.setup();
    render(<NewChatDialog />);
    await settle();
    await user.selectOptions(screen.getByRole('combobox'), 'claude-opus-4-7');
    expect(effortButtons()).toEqual(['medium', 'xhigh', 'ultracode']);
    await user.click(screen.getByRole('button', { name: 'Create' }));
    const [chat] = allChats(useChats.getState().folders);
    expect(chat).toMatchObject({ provider: 'claude', model: 'claude-opus-4-7', effort: 'xhigh' });
  });

  it('moves a hand-picked effort the next model lacks to its default, and runs a model without effort with none', async () => {
    const user = userEvent.setup();
    render(<NewChatDialog />);
    await settle();
    await user.click(screen.getByRole('radio', { name: 'max' }));
    await user.selectOptions(screen.getByRole('combobox'), 'claude-opus-4-7');
    expect(screen.getByRole('radio', { name: 'xhigh' })).toHaveAttribute('aria-checked', 'true');
    await user.selectOptions(screen.getByRole('combobox'), 'claude-haiku-4-5-20251001');
    await user.click(screen.getByRole('button', { name: 'Create' }));
    const [chat] = allChats(useChats.getState().folders);
    expect(chat).toMatchObject({ model: 'claude-haiku-4-5-20251001', effort: '' });
  });

  it('keeps a hand-picked effort the next model takes', async () => {
    const user = userEvent.setup();
    render(<NewChatDialog />);
    await settle();
    await user.click(screen.getByRole('radio', { name: 'low' }));
    await user.selectOptions(screen.getByRole('combobox'), 'Mythos');
    expect(screen.getByRole('radio', { name: 'low' })).toHaveAttribute('aria-checked', 'true');
  });

  it('opens on a full id the settings file names, as that model', async () => {
    mocked.claudeDefaults.mockResolvedValue({
      model: fromAccount('claude-opus-4-7'),
      effort: fromAccount('medium'),
      permissionMode: null
    });
    render(<NewChatDialog />);
    await settle();
    expect(screen.getByRole('combobox')).toHaveValue('claude-opus-4-7');
    expect(effortButtons()).toEqual(['medium', 'xhigh', 'ultracode']);
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

describe('NewChatDialog: where the wait guard starts', () => {
  const guardBox = () => screen.getByRole('checkbox', { name: 'Wait guard' });

  it('starts as Settings has it, and makes the chat so', async () => {
    useNewChat.getState().setDefault('guard', true);
    const user = userEvent.setup();
    render(<NewChatDialog />);
    expect(guardBox()).toBeChecked();
    expect(screen.getByText('as set in Settings')).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Create' }));
    expect(allChats(useChats.getState().folders)[0]).toMatchObject({ guard: true });
  });

  it('is kept for the folder with the other boxes, and back to that start; the defaults anyway keep nothing', async () => {
    const user = userEvent.setup();
    render(<NewChatDialog />);
    expect(guardBox()).not.toBeChecked();
    await user.click(guardBox());
    expect(screen.getByText('back to the default')).toBeInTheDocument();
    await user.click(screen.getByText('keep for this folder'));
    const startOfRepo = () => useChats.getState().folders.find(f => f.path === '/repo')?.start;
    expect(startOfRepo()).toEqual({ worktree: true, tools: false, guard: true });
    expect(screen.getByText('as kept for this folder')).toBeInTheDocument();

    await user.click(guardBox());
    await user.click(screen.getByText('back to this folder’s start'));
    expect(guardBox()).toBeChecked();

    // Flipped back to what Settings says, the folder has no start of its own.
    await user.click(guardBox());
    await user.click(screen.getByText('keep for this folder'));
    expect(startOfRepo()).toBeUndefined();
    expect(screen.getByText('as set in Settings')).toBeInTheDocument();
  });

  it('starts afresh in another folder, as that folder starts', async () => {
    useChats.getState().setStart('/repo', { worktree: true, tools: false, guard: true });
    useChats.getState().rememberFolder('/other');
    const user = userEvent.setup();
    render(<NewChatDialog />);
    expect(guardBox()).toBeChecked();
    await user.click(screen.getByTitle('/other'));
    expect(guardBox()).not.toBeChecked();
    await user.click(screen.getByTitle('/repo'));
    expect(guardBox()).toBeChecked();
  });

  it('follows the default where a folder’s start was kept before it had the guard', () => {
    // As an earlier Luna stored it: the two boxes it knew, no guard.
    useChats.getState().setStart('/repo', { worktree: false, tools: true });
    useNewChat.getState().setDefault('guard', true);
    render(<NewChatDialog />);
    expect(guardBox()).toBeChecked();
    expect(screen.getByRole('checkbox', { name: 'Git worktree' })).not.toBeChecked();
    expect(screen.getByText('as kept for this folder')).toBeInTheDocument();
  });

  it('shows off on Codex without hooks unasked, whatever the start says, and makes the chat so', async () => {
    useNewChat.getState().setDefault('guard', true);
    useNewChat.getState().remember('/repo', { provider: 'codex', name: 'gpt' });
    const user = userEvent.setup();
    render(<NewChatDialog />);
    await settle();
    expect(guardBox()).not.toBeChecked();
    expect(guardBox()).toBeDisabled();
    // Nothing on screen differs from the start, so nothing says it does.
    expect(screen.getByText('as set in Settings')).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Create' }));
    expect(allChats(useChats.getState().folders)[0]).toMatchObject({ hooksUnasked: false, guard: false });
  });

  it('leaves a guard Codex could not show to Settings when another box is kept', async () => {
    // A start an earlier Luna kept, which says nothing of the guard.
    useChats.getState().setStart('/repo', { worktree: false, tools: false });
    useNewChat.getState().remember('/repo', { provider: 'codex', name: 'gpt' });
    const user = userEvent.setup();
    const first = render(<NewChatDialog />);
    await settle();
    await user.click(screen.getByRole('checkbox', { name: 'Luna tools' }));
    await user.click(screen.getByText('keep for this folder'));
    const kept = useChats.getState().folders.find(f => f.path === '/repo')?.start;
    expect(kept).toMatchObject({ worktree: false, tools: true });
    expect(kept?.guard).toBeUndefined();
    first.unmount();

    // Settings turns the guard on: a Claude Code chat there follows it.
    useNewChat.getState().setDefault('guard', true);
    useNewChat.getState().remember('/repo', { provider: 'claude', name: 'work' });
    useNewChat.getState().openDialog('/repo');
    render(<NewChatDialog />);
    expect(guardBox()).toBeChecked();
    expect(screen.getByRole('checkbox', { name: 'Luna tools' })).toBeChecked();
  });

  it('takes the start on Codex once its hooks run unasked', async () => {
    useNewChat.getState().setDefault('guard', true);
    useNewChat.getState().remember('/repo', { provider: 'codex', name: 'gpt' });
    const user = userEvent.setup();
    render(<NewChatDialog />);
    await settle();
    await user.click(screen.getByRole('checkbox', { name: 'Hooks unasked' }));
    expect(guardBox()).toBeChecked();
    await user.click(screen.getByRole('button', { name: 'Create' }));
    expect(allChats(useChats.getState().folders)[0]).toMatchObject({ hooksUnasked: true, guard: true });
  });
});

describe('quick chat: the wait guard', () => {
  it('starts as the folder’s start has it', async () => {
    useChats.getState().setStart('/repo', { worktree: false, tools: false, guard: true });
    useNewChat.getState().remember('/repo', { provider: 'claude', name: 'work' });
    await quickChat();
    expect(allChats(useChats.getState().folders)[0]).toMatchObject({ provider: 'claude', guard: true });
  });

  it('on Codex, only with the folder’s hooks unasked', async () => {
    useNewChat.getState().setDefault('guard', true);
    useNewChat.getState().remember('/repo', { provider: 'codex', name: 'gpt' });
    await quickChat();
    vi.mocked(trust.codexHooksStart).mockResolvedValueOnce({ default: false, kept: true });
    await quickChat();
    const [without, unasked] = allChats(useChats.getState().folders);
    expect(without).toMatchObject({ hooksUnasked: false, guard: false });
    expect(unasked).toMatchObject({ hooksUnasked: true, guard: true });
  });
});
