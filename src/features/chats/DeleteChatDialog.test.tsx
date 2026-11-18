import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { backgroundDelete, backgroundMove, backgroundMoveTargets, type BackgroundHelperDto } from '../../ipc/agents';
import { deleteSession } from '../../ipc/worktrees';
import type { Chat } from '../../shared/types';
import { useBackground } from '../agents/background.store';
import { useChats } from './chats.store';
import { DeleteChatDialog } from './DeleteChatDialog';
import { useKeptWorktrees } from './keptWorktrees.store';

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
    expect(
      screen.getByText(/own helpers answer to no chat from then on, and each deletes itself half an hour/)
    ).toBeTruthy();
    expect(screen.getByText(/the helpers under one stay under it/)).toBeTruthy();
    // A chat it may not go to: its whole why, which an option cannot show.
    expect(screen.getAllByText(`Bare: chat "Bare" (bare) runs without Luna's tools`)).toHaveLength(3);
    expect(backgroundMove).not.toHaveBeenCalled();
  });

  it('keeps each preselection as first shown, whatever the rows say after', async () => {
    render(<DeleteChatDialog chat={release} onClose={vi.fn()} />);
    await waitFor(() => expect(picker('watched').value).toBe('ops'));
    // Ops' wait ends, and another chat asks for the plain one.
    act(() =>
      useBackground.setState({
        helpers: [
          row('asked', { suggested: [{ chat: 'docs', atMs: 1 }] }),
          row('kid', { parent: 'asked' }),
          row('watched'),
          row('plain', { suggested: [{ chat: 'docs', atMs: 2 }] })
        ]
      })
    );
    expect(picker('watched').value).toBe('ops');
    expect(picker('plain').value).toBe('');
  });

  it('prefers the chat that asked last, by when it asked', async () => {
    useBackground.setState({
      helpers: [
        row('asked', {
          suggested: [
            { chat: 'docs', atMs: 9 },
            { chat: 'ops', atMs: 3 }
          ]
        })
      ]
    });
    render(<DeleteChatDialog chat={release} onClose={vi.fn()} />);
    await waitFor(() => expect(picker('asked').value).toBe('docs'));
  });

  it('waits for where the helpers may go before Delete does anything', async () => {
    let answer: (t: { targets: never[] }) => void = () => {};
    vi.mocked(backgroundMoveTargets).mockImplementationOnce(() => new Promise(yes => (answer = yes)));
    useBackground.setState({ helpers: [row('plain')] });
    render(<DeleteChatDialog chat={release} onClose={vi.fn()} />);
    expect(screen.getByRole('button', { name: 'Delete' })).toBeDisabled();
    act(() => answer({ targets: [] }));
    await waitFor(() => expect(screen.getByRole('button', { name: 'Delete' })).toBeEnabled());
  });

  it('says why a helper held back cannot be passed on', async () => {
    vi.mocked(backgroundMoveTargets).mockResolvedValueOnce({
      blocked: 'plain is held back (opening as a chat); not moved',
      targets: [{ id: 'docs', ok: true }]
    });
    useBackground.setState({ helpers: [row('plain', { suggested: [{ chat: 'docs', atMs: 1 }] })] });
    render(<DeleteChatDialog chat={release} onClose={vi.fn()} />);
    expect(await screen.findByText(/Cannot be passed to a chat now: plain is held back/)).toBeTruthy();
    expect(screen.queryByRole('combobox')).toBeNull();
    expect(screen.getByRole('button', { name: 'Delete' })).toBeEnabled();
  });

  it('freezes its choices while the moves run: no box, picker, Enter or Cancel changes what happens', async () => {
    const user = userEvent.setup();
    let done: () => void = () => {};
    vi.mocked(backgroundMove).mockImplementationOnce(
      () =>
        new Promise(yes => (done = () => yes({ id: 'asked', from: 'chat', to: 'docs', sessions: ['asked', 'kid'] })))
    );
    const onClose = vi.fn();
    render(<DeleteChatDialog chat={release} onClose={onClose} />);
    await waitFor(() => expect(picker('asked').value).toBe('docs'));
    await user.selectOptions(picker('watched'), '');
    await user.click(screen.getByLabelText(/Also delete the 2 helpers/));
    await user.click(screen.getByRole('button', { name: 'Delete' }));
    expect(screen.getByRole('button', { name: 'Moving…' })).toBeDisabled();
    expect(screen.getByLabelText(/Also delete the 2 helpers/)).toBeDisabled();
    expect(picker('watched')).toBeDisabled();
    screen.getByRole('dialog').focus();
    await user.keyboard('{Enter}');
    await user.keyboard('{Escape}');
    await user.click(screen.getByRole('button', { name: 'Cancel' }));
    expect(onClose).not.toHaveBeenCalled();
    expect(backgroundMove).toHaveBeenCalledTimes(1);
    done();
    await waitFor(() => expect(deleteChat).toHaveBeenCalledWith('chat'));
    expect(vi.mocked(backgroundDelete).mock.calls.map(c => c[0])).toEqual(['watched', 'plain']);
  });

  it('deletes only what is still the chat’s after the moves, and tells the core whose it was', async () => {
    const user = userEvent.setup();
    vi.mocked(backgroundMove).mockImplementationOnce(() => {
      // Meanwhile another window moves "plain" to Ops; the move just made is
      // not in the rows yet.
      useBackground.setState({
        helpers: [
          row('asked', { suggested: [{ chat: 'docs', atMs: 1 }] }),
          row('kid', { parent: 'asked' }),
          row('watched', { watchedBy: ['ops'] }),
          row('plain', { parent: 'ops', top: 'ops' })
        ]
      });
      return Promise.resolve({ id: 'asked', from: 'chat', to: 'docs', sessions: ['asked', 'kid'] });
    });
    render(<DeleteChatDialog chat={release} onClose={vi.fn()} />);
    await waitFor(() => expect(picker('asked').value).toBe('docs'));
    await user.selectOptions(picker('watched'), '');
    await user.click(screen.getByLabelText(/Also delete the 2 helpers/));
    await user.click(screen.getByRole('button', { name: 'Delete' }));
    await waitFor(() => expect(deleteChat).toHaveBeenCalledWith('chat'));
    expect(vi.mocked(backgroundDelete).mock.calls).toEqual([
      ['watched', { id: 'watched', dropWorktree: true, stopWorking: false, dropUnsaved: false, ofChat: 'chat' }]
    ]);
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

  it('deletes nothing when a move is refused, says which helper, why and what moved already, and goes on from there', async () => {
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

    // The rows catch up: "asked" and its helper are Docs' now. Delete again
    // moves only what is left to move, and deletes nothing that moved.
    act(() =>
      useBackground.setState({
        helpers: [
          row('asked', { parent: 'docs', top: 'docs' }),
          row('kid', { parent: 'asked', top: 'docs' }),
          row('watched', { watchedBy: ['ops'] }),
          row('plain')
        ]
      })
    );
    vi.mocked(backgroundMove).mockClear();
    await user.click(screen.getByLabelText(/Also delete the 1 helper/));
    await user.click(screen.getByRole('button', { name: 'Delete' }));
    await waitFor(() => expect(deleteChat).toHaveBeenCalledWith('chat'));
    expect(vi.mocked(backgroundMove).mock.calls).toEqual([['watched', 'ops']]);
    expect(vi.mocked(backgroundDelete).mock.calls.map(c => c[0])).toEqual(['plain']);
    expect(onClose).toHaveBeenCalled();
  });

  it('says on the status bar when the worktree it was asked to take stayed', async () => {
    const user = userEvent.setup();
    useKeptWorktrees.setState({ paths: [] });
    vi.mocked(deleteSession).mockResolvedValueOnce({ worktree: '/repo/.claude/worktrees/w', worktreeKept: true });
    render(<DeleteChatDialog chat={chat('docs', 'Docs')} onClose={vi.fn()} />);
    await user.click(screen.getByRole('button', { name: 'Delete' }));
    await waitFor(() => expect(useKeptWorktrees.getState().paths).toEqual(['/repo/.claude/worktrees/w']));
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
