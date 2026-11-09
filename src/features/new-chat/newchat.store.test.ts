import { describe, expect, it, vi } from 'vitest';
import type { Folder } from '../../shared/types';
import { useChats } from '../chats/chats.store';
import { startFor, useNewChat } from './newchat.store';

// The chats store reaches the terminals; xterm cannot run in jsdom.
vi.mock('../panes/terminals', () => ({
  startSession: vi.fn(),
  focusTerminal: vi.fn(),
  forget: vi.fn(),
  forgetAll: vi.fn()
}));

const folder = (start?: Folder['start']): Folder => ({ id: 'f1', path: '/repo', open: true, chats: [], start });
const stored = (key: string, state: unknown, version: number) =>
  localStorage.setItem(key, JSON.stringify({ state, version }));

describe('new-chat store: where the boxes start', () => {
  it('starts every box as Settings has it in a folder that kept nothing', () => {
    expect(startFor(folder())).toEqual({ worktree: true, tools: false, guard: false });
    useNewChat.getState().setDefault('guard', true);
    expect(startFor(undefined)).toEqual({ worktree: true, tools: false, guard: true });
  });

  it('starts them as the folder kept them, the guard included', () => {
    useNewChat.getState().setDefault('guard', true);
    expect(startFor(folder({ worktree: false, tools: true, guard: false }))).toEqual({
      worktree: false,
      tools: true,
      guard: false
    });
  });

  it('takes the default guard where a start was kept before it had one', () => {
    useNewChat.getState().setDefault('guard', true);
    expect(startFor(folder({ worktree: false, tools: true }))).toEqual({ worktree: false, tools: true, guard: true });
  });
});

describe('new-chat store: persistence', () => {
  it('stores the guard’s default with the others', () => {
    useNewChat.getState().setDefault('guard', true);
    const saved = JSON.parse(localStorage.getItem('luna.newchat')!) as { state: Record<string, unknown> };
    expect(saved.state).toMatchObject({ worktree: true, tools: false, guard: true });
  });

  it('reads a state stored before the guard had a default as off, the rest as stored', async () => {
    stored('luna.newchat', { lastFolder: '/repo', lastAccount: null, worktree: false, tools: true }, 3);
    await useNewChat.persist.rehydrate();
    expect(useNewChat.getState()).toMatchObject({ lastFolder: '/repo', worktree: false, tools: true, guard: false });
  });

  it('reads a folder’s start stored without the guard, which then follows the default', async () => {
    stored('luna.chats', { folders: [{ ...folder({ worktree: false, tools: true }), picked: true }] }, 2);
    await useChats.persist.rehydrate();
    useNewChat.getState().setDefault('guard', true);
    const kept = useChats.getState().folders.find(f => f.path === '/repo');
    expect(kept?.start).toEqual({ worktree: false, tools: true });
    expect(startFor(kept)).toEqual({ worktree: false, tools: true, guard: true });
  });
});
