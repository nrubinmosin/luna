import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import type { Chat, Folder } from '../../shared/types';
import { AppliedPanel } from './AppliedPanel';
import { allChats, decideGuard, useChats } from './chats.store';
import { GuardNotice } from './GuardNotice';

vi.mock('../../ipc/applied', async importOriginal => ({
  ...(await importOriginal<typeof import('../../ipc/applied')>()),
  appliedByLuna: vi.fn(() => Promise.resolve({ launch: [], account: [], incomplete: null }))
}));
// xterm cannot run in jsdom.
vi.mock('../panes/terminals', () => ({
  startSession: vi.fn(),
  focusTerminal: vi.fn(),
  forget: vi.fn(),
  forgetAll: vi.fn()
}));

const chat = (id: string, extra: Partial<Chat>): Chat =>
  ({
    id,
    name: id,
    provider: 'claude',
    account: 'work',
    group: 0,
    worktree: false,
    status: 'resting',
    ...extra
  }) as Chat;
const folder = (chats: Chat[]): Folder => ({ id: 'f1', path: '/repo', open: true, chats, picked: true });

describe('the wait guard of chats made before it was a choice', () => {
  it('is off for every chat that did not say, and those that had it through Luna tools are named once', () => {
    const { folders, unguarded } = decideGuard([
      folder([
        chat('tooled', { tools: true }),
        chat('plain', {}),
        // A helper an earlier Luna nested under its parent had the guard as
        // a helper; it is the person's chat now and starts without it.
        chat('helper', { tools: true, parentId: 'tooled' } as Partial<Chat>),
        chat('chosen', { tools: true, guard: true })
      ])
    ]);
    expect(unguarded).toEqual(['tooled', 'helper']);
    const by = Object.fromEntries(allChats(folders).map(c => [c.id, c.guard]));
    expect(by).toEqual({ tooled: false, plain: false, helper: false, chosen: true });
    // Decided once: a second look finds nothing left to say.
    expect(decideGuard(folders).unguarded).toEqual([]);
  });

  it('is said in a notice until dismissed', async () => {
    useChats.setState({
      folders: [folder([chat('tooled', { tools: true, guard: false, name: 'fix the build' })])],
      guardNotice: ['tooled']
    });
    const user = userEvent.setup();
    render(<GuardNotice />);
    expect(screen.getByRole('status')).toHaveTextContent('fix the build had it because of Luna tools');
    await user.click(screen.getByRole('button', { name: 'Dismiss' }));
    expect(useChats.getState().guardNotice).toEqual([]);
    expect(screen.queryByRole('status')).toBeNull();
  });

  it('is turned on for an existing chat from its panel', async () => {
    const c = chat('c1', { tools: true, guard: false });
    useChats.setState({ folders: [folder([c])], guardNotice: [] });
    const user = userEvent.setup();
    render(<AppliedPanel chat={c} onClose={() => {}} />);
    await user.click(screen.getByRole('checkbox', { name: 'Wait guard for this chat' }));
    expect(useChats.getState().findChat('c1')?.guard).toBe(true);
  });

  it('has no usable box on Codex without its hooks unasked', () => {
    const codex = chat('x1', { provider: 'codex', hooksUnasked: false });
    render(<AppliedPanel chat={codex} onClose={() => {}} />);
    expect(screen.getByRole('checkbox', { name: 'Wait guard for this chat' })).toBeDisabled();
  });
});
