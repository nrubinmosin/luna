import { afterEach, describe, expect, it, vi } from 'vitest';
import type { Chat, Folder } from '../../shared/types';
import { useRestarts } from '../panes/restarts.store';
import { chatsChanged, foldersOfGroup, useChats } from './chats.store';
import { useMoveHints } from './moveHints.store';

// The chats store reaches the terminals; xterm cannot run in jsdom.
vi.mock('../panes/terminals', () => ({
  startSession: vi.fn(),
  focusTerminal: vi.fn(),
  forget: vi.fn(),
  forgetAll: vi.fn()
}));

const chat = (id: string, more: Partial<Chat> = {}): Chat =>
  ({
    id,
    name: id,
    status: 'resting',
    context: 0,
    account: 'work',
    group: 0,
    worktree: false,
    provider: 'claude',
    model: 'Opus',
    effort: 'high',
    perm: 'Bypass',
    ...more
  }) as Chat;

const seed = (...folders: Folder[]) => useChats.setState({ folders });
const folder = (id: string, chats: Chat[]): Folder => ({ id, path: `/${id}`, open: true, chats });
const saved = () =>
  JSON.parse(localStorage.getItem('luna.chats') ?? 'null') as {
    state: { folders: Folder[] };
    version: number;
  } | null;

afterEach(() => {
  vi.useRealTimers();
});

describe('chats store: a set that changes nothing', () => {
  it('leaves the state as it was', () => {
    seed(folder('a', [chat('c1', { status: 'working', sessionId: 's', worktreePath: '/w', context: 0.5 })]));
    const before = useChats.getState();
    const s = useChats.getState();
    s.setStatus('c1', 'working');
    s.setSessionId('c1', 's');
    s.setWorktreePath('c1', '/w');
    s.setContext('c1', 0.5, null, null);
    s.setModelSeen('c1', 'gpt');
    s.setStatus('missing', 'waiting');
    expect(useChats.getState()).toBe(before);
  });

  it('changes only the folder the chat is in', () => {
    const other = folder('b', [chat('c2')]);
    seed(folder('a', [chat('c1')]), other);
    useChats.getState().setStatus('c1', 'waiting');
    const now = useChats.getState().folders;
    expect(now[0]!.chats[0]!.status).toBe('waiting');
    expect(now[1]).toBe(other);
  });

  it('keeps the other folders as they were for every edit of one chat, and a delete', () => {
    const other = folder('b', [chat('c2')]);
    seed(folder('a', [chat('c1'), chat('c3')]), other);
    const s = useChats.getState();
    s.setName('c1', 'a title', 'cli');
    s.renameChat('c1', 'mine');
    s.setGuard('c1', true);
    s.setAutoHandoff('c1', true, 0.8);
    s.setHandoffMark('c1', { n: 1, at: 1, state: 'underWay', previous: false });
    s.setHandoffAsked('c1', { kind: 'manual', at: 2 } as never);
    s.setHandoffAsk('c1', { kind: 'manual', at: 3 } as never);
    s.setMoved('c1', { movedTo: 'c9' });
    s.setColor('c1', '#123456');
    s.setBrowser('c1', 'edge');
    s.deleteChat('c3');
    const now = useChats.getState().folders;
    expect(now[1]).toBe(other);
    expect(now[0]!.chats.map(c => c.id)).toEqual(['c1']);
    expect(now[0]!.chats[0]).toMatchObject({ name: 'mine', nameSource: 'user', guard: true, color: '#123456' });
    // The same color again is no change at all.
    const before = useChats.getState();
    s.setColor('c1', '#123456');
    s.setGuard('c1', true);
    expect(useChats.getState()).toBe(before);
  });
});

describe('chats store: one group of the folders', () => {
  it('gives back the same folder objects while the folders are unchanged', () => {
    seed(folder('a', [chat('c1'), chat('c2', { group: 1 })]), folder('b', [chat('c3')]));
    const first = foldersOfGroup(useChats.getState().folders, 0);
    useChats.getState().setStatus('c3', 'working');
    const second = foldersOfGroup(useChats.getState().folders, 0);
    expect(second[0]).toBe(first[0]);
    expect(second[1]).not.toBe(first[1]);
    expect(second[0]!.chats.map(c => c.id)).toEqual(['c1']);
    expect(foldersOfGroup(useChats.getState().folders, 1).map(f => f.id)).toEqual(['a']);
  });
});

describe('chats store: what changed for a subscriber', () => {
  const byGroup = (was: Chat, now: Chat) => was.group === now.group;

  it('sees no change in a status or context tick, nor in another field it ignores', () => {
    seed(folder('a', [chat('c1'), chat('c2')]), folder('b', [chat('c3')]));
    const before = useChats.getState().folders;
    useChats.getState().setStatus('c1', 'working');
    useChats.getState().setContext('c3', 0.3, null, null);
    useChats.getState().setName('c2', 'a title', 'cli');
    expect(chatsChanged(before, useChats.getState().folders, byGroup)).toBe(false);
  });

  it('sees a chat that came, went or moved group', () => {
    seed(folder('a', [chat('c1'), chat('c2')]));
    let before = useChats.getState().folders;
    useChats.getState().addChat('/a', chat('c4'), true);
    expect(chatsChanged(before, useChats.getState().folders, byGroup)).toBe(true);
    before = useChats.getState().folders;
    useChats.getState().deleteChat('c4');
    expect(chatsChanged(before, useChats.getState().folders, byGroup)).toBe(true);
    before = useChats.getState().folders;
    useChats.setState(s => ({
      folders: s.folders.map(f => ({ ...f, chats: f.chats.map(c => (c.id === 'c2' ? { ...c, group: 1 } : c)) }))
    }));
    expect(chatsChanged(before, useChats.getState().folders, byGroup)).toBe(true);
    before = useChats.getState().folders;
    useChats.getState().addChat('/new', chat('c5'), true);
    expect(chatsChanged(before, useChats.getState().folders, byGroup)).toBe(true);
  });
});

describe('chats store: persistence', () => {
  it('stores a chat without its context, its status kept', () => {
    vi.useFakeTimers();
    seed(folder('a', [chat('c1')]));
    useChats.getState().setContext('c1', 0.4, 4000, 10000);
    useChats.getState().setStatus('c1', 'waiting');
    vi.advanceTimersByTime(1000);
    const c = saved()!.state.folders[0]!.chats[0]!;
    expect(c.status).toBe('waiting');
    expect(c).not.toHaveProperty('context');
    expect(c).not.toHaveProperty('contextTokens');
    expect(c).not.toHaveProperty('contextWindow');
  });

  it('writes a burst of changes once, with the last of them', () => {
    vi.useFakeTimers();
    const setItem = vi.spyOn(Storage.prototype, 'setItem');
    seed(folder('a', [chat('c1')]));
    const writes = () => setItem.mock.calls.filter(c => c[0] === 'luna.chats').length;
    const first = writes();
    for (const n of ['one', 'two', 'three']) useChats.getState().renameChat('c1', n);
    expect(writes()).toBe(first);
    vi.advanceTimersByTime(600);
    expect(writes()).toBe(first + 1);
    expect(saved()!.state.folders[0]!.chats[0]!.name).toBe('three');
    setItem.mockRestore();
  });

  it('writes a session id at once, after a status the same poll set', () => {
    vi.useFakeTimers();
    seed(folder('a', [chat('c1')]));
    vi.advanceTimersByTime(1000);
    useChats.getState().setStatus('c1', 'working');
    useChats.getState().setSessionId('c1', 'sess-1');
    const c = saved()!.state.folders[0]!.chats[0]!;
    expect(c.sessionId).toBe('sess-1');
    expect(c.status).toBe('working');
  });

  it('writes a new chat and a worktree at once, and the same session id again not at all', () => {
    vi.useFakeTimers();
    seed(folder('a', [chat('c1', { sessionId: 's' })]));
    vi.advanceTimersByTime(1000);
    useChats.getState().setStatus('c1', 'working');
    useChats.getState().addChat('/a', chat('c2'), true);
    expect(saved()!.state.folders[0]!.chats.map(c => c.id)).toEqual(['c1', 'c2']);
    useChats.getState().setWorktreePath('c2', '/w');
    expect(saved()!.state.folders[0]!.chats[1]!.worktreePath).toBe('/w');
    const setItem = vi.spyOn(Storage.prototype, 'setItem');
    useChats.getState().setStatus('c1', 'waiting');
    useChats.getState().setSessionId('c1', 's');
    expect(setItem.mock.calls.filter(c => c[0] === 'luna.chats')).toHaveLength(0);
    setItem.mockRestore();
  });

  it('loads chats stored with their context, and those stored without', async () => {
    localStorage.setItem(
      'luna.chats',
      JSON.stringify({
        state: {
          folders: [
            { ...folder('a', [chat('old', { context: 0.7 })]), picked: true },
            {
              id: 'b',
              path: '/b',
              open: true,
              picked: true,
              chats: [{ ...chat('new'), context: undefined }]
            }
          ],
          active: 'old'
        },
        version: 2
      })
    );
    await useChats.persist.rehydrate();
    expect(useChats.getState().findChat('old')?.context).toBe(0.7);
    expect(useChats.getState().findChat('new')?.context).toBe(0);
    expect(useChats.getState().active).toBe('old');
  });
});

describe('chats store: a chat deleted', () => {
  it('takes what this run kept on it with it', () => {
    seed(folder('a', [chat('c1'), chat('c2')]));
    useRestarts.getState().bump('c1');
    useRestarts.getState().setPending('c1', true);
    useRestarts.getState().bump('c2');
    useMoveHints.getState().setLimit('c1', 'usage_limit');
    useMoveHints.getState().dismiss('c1');
    useChats.getState().deleteChat('c1');
    expect(useRestarts.getState().by).toEqual({ c2: 1 });
    expect(useRestarts.getState().pending).toEqual({});
    expect(useMoveHints.getState().limits).toEqual({});
    expect(useMoveHints.getState().dismissed).toEqual({});
  });
});
