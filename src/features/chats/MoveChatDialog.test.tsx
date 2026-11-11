import { act, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import type { Account, Chat, Folder, LimitWindow } from '../../shared/types';
import { useAccounts } from '../accounts/accounts.store';
import { useChats } from './chats.store';
import { askRefusal, defaultTarget, MoveChatDialog } from './MoveChatDialog';
import { MoveHint } from './MoveHint';
import { useMoveHints } from './moveHints.store';
import { carryOut, launchFor, movedName } from './moving';
import { markWords } from './Handoff';
import * as moving from '../../ipc/moving';
import { createChat } from '../new-chat/create';
import type { MoveStatusDto } from '../../ipc/moving';

// xterm cannot run in jsdom.
vi.mock('../panes/terminals', () => ({
  startSession: vi.fn(),
  focusTerminal: vi.fn(),
  forget: vi.fn(),
  forgetAll: vi.fn()
}));
vi.mock('../../ipc/moving', async importOriginal => ({
  ...(await importOriginal<typeof import('../../ipc/moving')>()),
  moveAsk: vi.fn(),
  movePrepare: vi.fn(),
  moveConfirm: vi.fn(() => new Promise(() => {})),
  moveCancel: vi.fn(() => Promise.resolve({})),
  moveStep: vi.fn((chatId: string, moveId: string, step: string) =>
    Promise.resolve({ chatId, moveId, newId: 'c-new', phase: step, stopOld: false })
  ),
  moveList: vi.fn(() => Promise.resolve([]))
}));
vi.mock('../new-chat/create', () => ({ createChat: vi.fn(() => new Promise(r => setTimeout(() => r('c-new'), 20))) }));
vi.mock('../../ipc/trust', async importOriginal => ({
  ...(await importOriginal<typeof import('../../ipc/trust')>()),
  claudeFolderTrusted: vi.fn(() => Promise.resolve(true))
}));
vi.mock('../../ipc/defaults', async importOriginal => ({
  ...(await importOriginal<typeof import('../../ipc/defaults')>()),
  claudeModels: vi.fn(() => Promise.resolve([])),
  codexModels: vi.fn(() => Promise.resolve([]))
}));

const window_ = (used: number): LimitWindow => ({ id: 'w', label: '5 hours', short: '5H', used, resetAt: null });
const account = (name: string, provider: 'claude' | 'codex', used: number[] = [0.1]): Account => ({
  provider,
  name,
  path: `/acc/${provider}/${name}`,
  plan: 'max',
  email: null,
  signedIn: true,
  haveUsage: true,
  limits: used.map(window_),
  usageAge: null,
  fetchedAt: null,
  sync: 'ready',
  refreshError: null
});
const chat = (extra: Partial<Chat>): Chat =>
  ({
    id: 'c1',
    name: 'Campaign',
    provider: 'claude',
    account: 'team',
    group: 0,
    worktree: false,
    status: 'resting',
    context: 0,
    model: 'Opus',
    effort: 'high',
    perm: 'Edits',
    ...extra
  }) as Chat;
const folder = (chats: Chat[]): Folder => ({ id: 'f1', path: '/repo', open: true, chats, picked: true });

describe('moving a chat to another account', () => {
  it('goes by default to the first other account of its CLI with every window under 95%', () => {
    const accounts = [
      account('team', 'claude'),
      account('busy', 'claude', [0.97]),
      account('codex-one', 'codex'),
      account('free', 'claude', [0.4, 0.94])
    ];
    expect(defaultTarget(accounts, chat({}))?.name).toBe('free');
    expect(defaultTarget([account('team', 'claude'), account('busy', 'claude', [0.99])], chat({}))?.name).toBe('busy');
    expect(defaultTarget([account('team', 'claude'), account('codex-one', 'codex')], chat({}))).toBeNull();
  });

  it('offers "Ask the chat" only to a running chat with auto-handoff that can write a file', () => {
    const on = { tools: true, autoHandoff: true };
    expect(askRefusal(chat(on), null)).toBeNull();
    expect(askRefusal(chat({}), null)).toMatch(/auto-handoff/);
    expect(askRefusal(chat({ ...on, perm: 'Plan only' }), null)).toMatch(/plan mode/);
    expect(askRefusal(chat({ ...on, provider: 'codex', sandbox: 'read-only' }), null)).toMatch(/read-only/);
    // At rest is no stopped CLI: an idle chat reads so. The core refuses one
    // whose CLI does not run.
    expect(askRefusal(chat({ ...on, status: 'resting' }), null)).toBeNull();
    // Its turn over, subagents at work: at its prompt, so it may be asked.
    expect(askRefusal(chat({ ...on, status: 'background' }), null)).toBeNull();
    expect(askRefusal(chat(on), 'usage_limit')).toMatch(/usage limit/);
  });

  it("starts the new chat with the old one's permissions, folder, look and worktree by name", () => {
    const to = account('backend', 'claude');
    const l = launchFor(
      chat({ color: 'teal', tools: true, autoHandoff: true, handoffAt: 70, readOnly: true }),
      '/repo',
      to,
      'Opus',
      'high',
      false
    );
    expect(l).toMatchObject({
      provider: 'claude',
      accountPath: to.path,
      folder: '/repo',
      perm: 'Edits',
      permissionMode: 'acceptEdits',
      readOnly: true,
      color: 'teal',
      name: 'Campaign (moved)',
      autoHandoff: true,
      handoffAt: 70,
      worktreeName: null
    });
    const wt = launchFor(
      chat({ worktree: true, worktreePath: '/repo/.claude/worktrees/mv-test' }),
      '/repo',
      to,
      'Opus',
      'high',
      true
    );
    expect(wt.worktreeName).toBe('mv-test');
    const codex = launchFor(
      chat({ provider: 'codex', approval: 'on-request', sandbox: 'workspace-write', model: null }),
      '/repo',
      account('two', 'codex'),
      null,
      'high',
      false
    );
    expect(codex).toMatchObject({ approval: 'on-request', sandbox: 'workspace-write', worktree: false });
    expect(movedName('X (moved)')).toBe('X (moved)');
  });

  it("does not move a Codex chat out of the worktree in its account's folder", () => {
    const c = chat({ provider: 'codex', worktree: true, worktreePath: '/acc/codex/one/worktrees/ab12/repo' });
    useChats.setState({ folders: [folder([c])] });
    useAccounts.setState({ accounts: [account('one', 'codex'), account('two', 'codex')] });
    render(<MoveChatDialog chat={c} onClose={() => {}} />);
    expect(screen.getByTestId('move-dialog')).toHaveTextContent('does not move');
    expect(screen.queryByRole('button', { name: /Preview/ })).toBeNull();
  });

  it('says why "Ask the chat" is not offered, and stops a Claude Code worktree\'s old chat always', () => {
    const c = chat({ worktree: true, worktreePath: '/repo/.claude/worktrees/mv' });
    useChats.setState({ folders: [folder([c])] });
    useAccounts.setState({ accounts: [account('team', 'claude'), account('backend', 'claude')] });
    render(<MoveChatDialog chat={c} onClose={() => {}} />);
    const ask = screen.getByRole('radio', { name: /Ask the chat/ });
    expect(ask).toBeDisabled();
    expect(screen.getByText(/Only a chat with auto-handoff on/)).toBeInTheDocument();
    expect(screen.getByRole('radio', { name: /Keep it/ })).toBeDisabled();
    expect(screen.getByRole('radio', { name: /Stop it/ })).toBeChecked();
  });

  it('offers a move from the pane when the account runs low or a limit stopped the chat, until dismissed', () => {
    const c = chat({});
    useAccounts.setState({ accounts: [account('team', 'claude', [0.92]), account('backend', 'claude')] });
    useMoveHints.setState({ limits: {}, dismissed: {} });
    const { rerender } = render(<MoveHint chat={c} onMove={() => {}} />);
    expect(screen.getByTestId('move-hint')).toHaveTextContent('team is at 92% of its 5 hours limit');
    useMoveHints.getState().dismiss('c1');
    rerender(<MoveHint chat={c} onMove={() => {}} />);
    expect(screen.queryByTestId('move-hint')).toBeNull();
    // A limit, and nowhere to go: nothing.
    useMoveHints.setState({ limits: { c1: 'usage_limit' }, dismissed: {} });
    useAccounts.setState({ accounts: [account('team', 'claude')] });
    rerender(<MoveHint chat={c} onMove={() => {}} />);
    expect(screen.queryByTestId('move-hint')).toBeNull();
    useAccounts.setState({ accounts: [account('team', 'claude'), account('backend', 'claude')] });
    rerender(<MoveHint chat={c} onMove={() => {}} />);
    expect(screen.getByTestId('move-hint')).toHaveTextContent('This chat stopped on a usage limit.');
  });

  it("makes a moved chat's row once, however many ask for it at once", async () => {
    useChats.setState({ folders: [folder([chat({})])] });
    useAccounts.setState({ accounts: [account('team', 'claude'), account('backend', 'claude')] });
    const launch = launchFor(chat({}), '/repo', account('backend', 'claude'), 'Opus', 'high', false);
    const m: MoveStatusDto = {
      moveId: 'm1',
      chatId: 'c1',
      newId: 'c-new',
      phase: 'confirmed',
      at: 1,
      stopOld: false,
      worktree: false,
      toAccount: 'backend',
      source: 'luna',
      launch
    };
    vi.mocked(createChat).mockClear();
    await Promise.all([carryOut(m), carryOut(m)]);
    expect(createChat).toHaveBeenCalledTimes(1);
    // And a row asked for twice is listed once.
    const row = { ...chat({}), id: 'c-twice' };
    useChats.getState().addChat('/repo', row, true);
    useChats.getState().addChat('/repo', row, true);
    expect(
      useChats
        .getState()
        .folders.flatMap(f => f.chats)
        .filter(c => c.id === 'c-twice')
    ).toHaveLength(1);
  });

  it('gives up an ask whose dialog closed while the core was still answering', async () => {
    const c = chat({ tools: true, autoHandoff: true });
    useChats.setState({ folders: [folder([c])] });
    useAccounts.setState({ accounts: [account('team', 'claude'), account('backend', 'claude')] });
    useMoveHints.setState({ limits: {}, dismissed: {} });
    let answer: (s: MoveStatusDto) => void = () => {};
    vi.mocked(moving.moveAsk).mockReturnValue(new Promise(r => (answer = r)));
    vi.mocked(moving.moveCancel).mockClear();
    const user = userEvent.setup();
    const { unmount } = render(<MoveChatDialog chat={c} onClose={() => {}} />);
    await user.click(screen.getByRole('radio', { name: /Ask the chat/ }));
    const ask = await screen.findByRole('button', { name: 'Ask the chat' });
    await vi.waitFor(() => expect(ask).toBeEnabled());
    await user.click(ask);
    unmount();
    await act(async () => {
      answer({
        moveId: 'm-late',
        chatId: 'c1',
        newId: 'c-new',
        phase: 'asking',
        at: 1,
        stopOld: false,
        worktree: false,
        toAccount: 'backend',
        source: 'chat'
      });
      await Promise.resolve();
    });
    expect(moving.moveCancel).toHaveBeenCalledWith('c1', 'm-late');
  });

  it('does not move a Codex chat started into a worktree before its first turn', () => {
    const c = chat({ provider: 'codex', worktree: true, worktreePath: null });
    useChats.setState({ folders: [folder([c])] });
    useAccounts.setState({ accounts: [account('one', 'codex'), account('two', 'codex')] });
    render(<MoveChatDialog chat={c} onClose={() => {}} />);
    expect(screen.getByTestId('move-dialog')).toHaveTextContent('does not move');
  });

  it('draws its choices the way XP.css draws them: each control before its label', () => {
    const c = chat({ tools: true, autoHandoff: true });
    useChats.setState({ folders: [folder([c])] });
    useAccounts.setState({ accounts: [account('team', 'claude'), account('backend', 'claude')] });
    render(<MoveChatDialog chat={c} onClose={() => {}} />);
    for (const name of [/Luna writes it/, /Ask the chat/, /Keep it/, /Stop it/]) {
      const radio = screen.getByRole('radio', { name });
      expect(radio.nextElementSibling?.tagName).toBe('LABEL');
      expect(radio.nextElementSibling?.getAttribute('for')).toBe(radio.id);
    }
    expect(document.querySelector('.move-dialog fieldset')).toBeNull();
  });

  it('leaves the panes in reach while the chat is asked, and its move is canceled from there', async () => {
    const c = chat({ tools: true, autoHandoff: true });
    useChats.setState({ folders: [folder([c])] });
    useAccounts.setState({ accounts: [account('team', 'claude'), account('backend', 'claude')] });
    vi.mocked(moving.moveAsk).mockResolvedValue({
      moveId: 'm-ask',
      chatId: 'c1',
      newId: 'c-new',
      phase: 'asking',
      at: 1,
      stopOld: false,
      worktree: false,
      toAccount: 'backend',
      source: 'chat'
    });
    vi.mocked(moving.moveCancel).mockClear();
    const user = userEvent.setup();
    const app = document.createElement('div');
    app.setAttribute('data-app', '');
    const panes = document.createElement('div');
    panes.setAttribute('data-testid', 'panes');
    app.appendChild(panes);
    document.body.appendChild(app);
    render(<MoveChatDialog chat={c} onClose={() => {}} />);
    await user.click(screen.getByRole('radio', { name: /Ask the chat/ }));
    const ask = screen.getByRole('button', { name: 'Ask the chat' });
    await vi.waitFor(() => expect(ask).toBeEnabled());
    await user.click(ask);
    const banner = await screen.findByTestId('move-banner');
    expect(document.querySelector('.scrim')).toBeNull();
    expect(screen.getByTestId('panes').closest('[inert]')).toBeNull();
    await user.click(screen.getByRole('button', { name: 'Cancel the move' }));
    expect(moving.moveCancel).toHaveBeenCalledWith('c1', 'm-ask');
    expect(banner).toBeTruthy();
    app.remove();
  });

  it('sends once for two clicks in one task', async () => {
    const c = chat({});
    useChats.setState({ folders: [folder([c])] });
    useAccounts.setState({ accounts: [account('team', 'claude'), account('backend', 'claude')] });
    vi.mocked(moving.movePrepare).mockResolvedValue({
      moveId: 'm-p',
      newId: 'c-new',
      prompt: '[Luna move m-p, not typed by the user] Read it.',
      text: '# Moved\n',
      sha256: '',
      redacted: 0,
      pathsCut: 0,
      invisible: 0,
      highlights: [],
      stopOld: false,
      worktree: false,
      warnings: [],
      maxBytes: 96 * 1024
    });
    vi.mocked(moving.moveConfirm).mockClear();
    const user = userEvent.setup();
    render(<MoveChatDialog chat={c} onClose={() => {}} />);
    const preview = screen.getByRole('button', { name: 'Preview' });
    await vi.waitFor(() => expect(preview).toBeEnabled());
    await user.click(preview);
    const send = await screen.findByRole('button', { name: /Send to backend/ });
    act(() => {
      send.click();
      send.click();
    });
    await vi.waitFor(() => expect(moving.moveConfirm).toHaveBeenCalledTimes(1));
    await new Promise(r => setTimeout(r, 50));
    expect(moving.moveConfirm).toHaveBeenCalledTimes(1);
  });

  it('says on a moved chat whose move failed why', () => {
    expect(
      markWords({ n: 1, at: 0, state: 'failed', moved: true, previous: false, why: 'no line within 60 s' })
    ).toMatch(/did not go through: no line within 60 s/);
  });

  it('leaves stopping the old chat to the core, which looks at its worktree first', async () => {
    useChats.setState({ folders: [folder([chat({})])] });
    useAccounts.setState({ accounts: [account('team', 'claude'), account('backend', 'claude')] });
    const launch = launchFor(chat({}), '/repo', account('backend', 'claude'), 'Opus', 'high', true);
    vi.mocked(moving.moveStep).mockClear();
    await carryOut({
      moveId: 'm-stop',
      chatId: 'c1',
      newId: 'c-stop',
      phase: 'confirmed',
      at: 1,
      stopOld: true,
      worktree: true,
      toAccount: 'backend',
      source: 'luna',
      launch
    });
    expect(vi.mocked(moving.moveStep).mock.calls[0]).toEqual(['c1', 'm-stop', 'stopOld']);
  });

  it('lays the strip over the terminal, taking none of its rows', () => {
    useAccounts.setState({ accounts: [account('team', 'claude', [0.95]), account('backend', 'claude')] });
    useMoveHints.setState({ limits: {}, dismissed: {} });
    render(<MoveHint chat={chat({})} onMove={() => {}} />);
    expect(screen.getByTestId('move-hint-slot').style.height).toBe('0px');
    expect(screen.getByTestId('move-hint').style.position).toBe('absolute');
  });

  it('marks a signed-out account and does not move a chat to it', async () => {
    const c = chat({});
    useChats.setState({ folders: [folder([c])] });
    const out = { ...account('backend', 'claude'), signedIn: false };
    useAccounts.setState({ accounts: [account('team', 'claude'), out] });
    expect(defaultTarget([account('team', 'claude'), out, account('spare', 'claude')], c)?.name).toBe('spare');
    render(<MoveChatDialog chat={c} onClose={() => {}} />);
    expect(await screen.findByRole('alert')).toHaveTextContent('backend is signed out');
    expect(screen.getByRole('button', { name: 'Preview' })).toBeDisabled();
  });
});
