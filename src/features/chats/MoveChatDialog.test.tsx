import { render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import type { Account, Chat, Folder, LimitWindow } from '../../shared/types';
import { useAccounts } from '../accounts/accounts.store';
import { useChats } from './chats.store';
import { askRefusal, defaultTarget, MoveChatDialog } from './MoveChatDialog';
import { MoveHint } from './MoveHint';
import { useMoveHints } from './moveHints.store';
import { launchFor, movedName } from './moving';

// xterm cannot run in jsdom.
vi.mock('../panes/terminals', () => ({
  startSession: vi.fn(),
  focusTerminal: vi.fn(),
  forget: vi.fn(),
  forgetAll: vi.fn()
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
    status: 'idle',
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
    expect(askRefusal(chat({ ...on, status: 'resting' }), null)).toMatch(/not running/);
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
});
