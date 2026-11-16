import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { backgroundDelete, backgroundMove, backgroundMoveTargets, type BackgroundHelperDto } from '../../ipc/agents';
import { deleteSession } from '../../ipc/worktrees';
import type { Chat } from '../../shared/types';
import { useBackground } from '../agents/background.store';
import { useChats } from './chats.store';
import { DeleteChatDialog } from './DeleteChatDialog';

// xterm cannot run in jsdom.
vi.mock('../panes/terminals', () => ({
  startSession: vi.fn(),
  focusTerminal: vi.fn(),
  forget: vi.fn(),
  forgetAll: vi.fn()
}));
vi.mock('../../ipc/agents', async importOriginal => ({
  ...(await importOriginal<typeof import('../../ipc/agents')>()),
  backgroundAtStake: vi.fn(() => Promise.resolve([])),
  backgroundDelete: vi.fn(() => Promise.resolve({})),
  backgroundMove: vi.fn(() => Promise.resolve({ id: 'x', from: 'chat', to: 'docs', sessions: ['x'] })),
  backgroundMoveTargets: vi.fn(() =>
    Promise.resolve({
      targets: [
        { id: 'chat', ok: false, why: 'a helper of chat "Release" already' },
        { id: 'docs', ok: true },
        { id: 'ops', ok: true },
        { id: 'bare', ok: false, why: `chat "Bare" (bare) runs without Luna's tools` }
      ]
    })
  )
}));
vi.mock('../../ipc/worktrees', async importOriginal => ({
  ...(await importOriginal<typeof import('../../ipc/worktrees')>()),
  deleteSession: vi.fn(() => Promise.resolve())
}));

const chat = (id: string, name: string) =>
  ({ id, name, provider: 'claude', group: 0, worktree: false, status: 'resting' }) as Chat;
const release = chat('chat', 'Release');

const row = (id: string, over: Partial<BackgroundHelperDto> = {}): BackgroundHelperDto => ({
  id,
  parent: 'chat',
  name: id,
  provider: 'claude',
  state: 'idle',
  account: 'work',
  model: null,
  effort: 'low',
  worktree: false,
  startedMs: 0,
  turnStartedMs: null,
  lastActivityMs: null,
  task: null,
  top: 'chat',
  ...over
});

const deleteChat = vi.fn();
beforeEach(() => {
  vi.clearAllMocks();
  useChats.setState({
    folders: [
      {
        id: 'f',
        path: '/repo',
        open: true,
        chats: [release, chat('docs', 'Docs'), chat('ops', 'Ops'), chat('bare', 'Bare')]
      }
    ],
    deleteChat
  });
  useBackground.setState({
    helpers: [
      row('asked', { suggested: [{ chat: 'docs', atMs: 1 }] }),
      row('kid', { parent: 'asked' }),
      row('watched', { watchedBy: ['ops'] }),
      row('plain')
    ]
  });
});

const picker = (name: string) => screen.getByRole<HTMLSelectElement>('combobox', { name: `Pass "${name}" to a chat` });

describe('DeleteChatDialog', () => {
  it('preselects a chat that asked for a helper or watches it, offers the rest, and moves nothing yet', async () => {
    render(<DeleteChatDialog chat={release} onClose={vi.fn()} />);
    await waitFor(() => expect(picker('asked').value).toBe('docs'));
    expect(picker('watched').value).toBe('ops');
    expect(picker('plain').value).toBe('');
    // The chat going is not offered; one without tools is, disabled.
    const options = [...picker('plain').options];
    expect(options.map(o => o.value)).toEqual(['', 'docs', 'ops', 'bare']);
    expect(options.find(o => o.value === 'bare')?.disabled).toBe(true);
    // A helper's helper has no picker of its own: it goes with the one above it.
    expect(screen.queryByRole('combobox', { name: 'Pass "kid" to a chat' })).toBeNull();
    expect(screen.getByText(/goes with asked/)).toBeTruthy();
    // Leaving a helper makes it one with no chat that deletes itself.
    expect(screen.getByText(/Also delete the 1 helper its agent runs that you do not pass to a chat/)).toBeTruthy();
    expect(screen.getByText(/answers to no chat from then on and deletes itself half an hour after/)).toBeTruthy();
    expect(backgroundMove).not.toHaveBeenCalled();
  });

  it('moves the passed helpers before it deletes the chat, and deletes none of them', async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();
    render(<DeleteChatDialog chat={release} onClose={onClose} />);
    await waitFor(() => expect(picker('asked').value).toBe('docs'));
    // The person takes back the preselection of one, and ticks the box for the rest.
    await user.selectOptions(picker('watched'), '');
    await user.click(screen.getByLabelText(/Also delete the 2 helpers/));
    await user.click(screen.getByRole('button', { name: 'Delete' }));
    await waitFor(() => expect(deleteChat).toHaveBeenCalledWith('chat'));
    expect(backgroundMove).toHaveBeenCalledTimes(1);
    expect(backgroundMove).toHaveBeenCalledWith('asked', 'docs');
    expect(vi.mocked(backgroundMove).mock.invocationCallOrder[0]).toBeLessThan(
      vi.mocked(deleteSession).mock.invocationCallOrder[0] ?? 0
    );
    // The passed one and its helper stay; the others go with the chat.
    expect(vi.mocked(backgroundDelete).mock.calls.map(c => c[0])).toEqual(['watched', 'plain']);
    expect(onClose).toHaveBeenCalled();
  });

  it('deletes nothing when a move is refused, and says which helper and why', async () => {
    const user = userEvent.setup();
    vi.mocked(backgroundMove)
      .mockResolvedValueOnce({ id: 'asked', from: 'chat', to: 'docs', sessions: ['asked', 'kid'] })
      .mockRejectedValueOnce('chat "Ops" (ops) has 8 of its 8 places in use, and watched would bring 1: not moved');
    const onClose = vi.fn();
    render(<DeleteChatDialog chat={release} onClose={onClose} />);
    await waitFor(() => expect(picker('watched').value).toBe('ops'));
    await user.click(screen.getByRole('button', { name: 'Delete' }));
    const alert = await screen.findByRole('alert');
    expect(alert).toHaveTextContent('Nothing was deleted: "watched" did not move to "Ops"');
    expect(alert).toHaveTextContent('8 of its 8 places in use');
    expect(alert).toHaveTextContent('(asked moved already.)');
    expect(deleteSession).not.toHaveBeenCalled();
    expect(deleteChat).not.toHaveBeenCalled();
    expect(backgroundDelete).not.toHaveBeenCalled();
    expect(onClose).not.toHaveBeenCalled();
    expect(screen.getByRole('button', { name: 'Delete' })).toBeEnabled();
  });

  it('asks where only the chat’s own helpers may go', async () => {
    render(<DeleteChatDialog chat={release} onClose={vi.fn()} />);
    await waitFor(() => expect(backgroundMoveTargets).toHaveBeenCalledTimes(3));
    expect(
      vi
        .mocked(backgroundMoveTargets)
        .mock.calls.map(c => c[0])
        .sort()
    ).toEqual(['asked', 'plain', 'watched']);
  });
});
