import { renderHook, waitFor } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import * as sessionsIpc from '../../ipc/sessions';
import type { Account, Chat } from '../../shared/types';
import { useAccounts } from '../accounts/accounts.store';
import { useChats } from './chats.store';
import { mapStatus, useSessionWatch } from './useSessionWatch';

vi.mock('../../ipc/sessions', async importOriginal => ({
  ...(await importOriginal<typeof import('../../ipc/sessions')>()),
  // No CLI running: nothing of it to say, and no handoff asked.
  sessionsMeta: vi.fn((chats: { id: string }[]) =>
    Promise.resolve(chats.map(c => ({ id: c.id, meta: null, handoffAsk: null })))
  ),
  savedTitle: vi.fn(() => Promise.resolve(null))
}));

const account = { provider: 'claude', name: 'work', path: '/accounts/claude/work' } as Account;

const chatOf = (id: string, more: Partial<Chat> = {}) =>
  ({ id, name: 'campaign', provider: 'claude', account: 'work', status: 'resting', tools: true, ...more }) as Chat;

/** One tick of the watch over `chats`, done once the core has been asked. */
async function tickOver(chats: Chat[]) {
  useAccounts.setState({ accounts: [account], loaded: true });
  useChats.setState({ folders: [{ id: 'f', path: '/repo', open: true, chats }] });
  const before = vi.mocked(sessionsIpc.sessionsMeta).mock.calls.length;
  const { unmount } = renderHook(() => useSessionWatch());
  await waitFor(() => expect(vi.mocked(sessionsIpc.sessionsMeta).mock.calls.length).toBe(before + 1));
  // The tick's own awaits after the answer.
  await new Promise(r => setTimeout(r, 0));
  unmount();
}

describe('mapStatus', () => {
  it('takes the core’s word for work beside the turn as it is', () => {
    expect(mapStatus('background')).toBe('background');
  });

  it('folds the CLIs’ words as ever', () => {
    expect(mapStatus('busy')).toBe('working');
    expect(mapStatus('working')).toBe('working');
    expect(mapStatus('waiting')).toBe('waiting');
    expect(mapStatus('needs permission')).toBe('waiting');
    expect(mapStatus('idle')).toBe('resting');
    expect(mapStatus(null)).toBe('resting');
  });
});

describe('useSessionWatch', () => {
  it("takes the core's word on a handoff ask while the chat's CLI is not running", async () => {
    const chat = chatOf('w1', { handoffAsk: { kind: 'manual', at: 1000 } });
    useAccounts.setState({ accounts: [account], loaded: true });
    useChats.setState({ folders: [{ id: 'f', path: '/repo', open: true, chats: [chat] }] });
    const { unmount } = renderHook(() => useSessionWatch());
    // The core lapsed it while the CLI was down: the chip and the menu's
    // refusal go with it.
    await waitFor(() => expect(useChats.getState().findChat('w1')?.handoffAsk ?? null).toBeNull());
    expect(sessionsIpc.sessionsMeta).toHaveBeenCalledWith([{ id: 'w1', accountPath: '/accounts/claude/work' }]);
    unmount();
  });

  it('asks the core about every chat with an account in one call', async () => {
    await tickOver([chatOf('m1'), chatOf('m2', { account: 'gone' }), chatOf('m3')]);
    expect(vi.mocked(sessionsIpc.sessionsMeta).mock.lastCall?.[0]).toEqual([
      { id: 'm1', accountPath: '/accounts/claude/work' },
      { id: 'm3', accountPath: '/accounts/claude/work' }
    ]);
  });

  it('takes every chat for resting when the core cannot be asked, its handoff ask left as it was', async () => {
    vi.mocked(sessionsIpc.sessionsMeta).mockRejectedValueOnce(new Error('the core is not running'));
    const ask = { kind: 'manual', at: 1000 } as const;
    await tickOver([chatOf('f1', { status: 'working', handoffAsk: ask }), chatOf('f2', { status: 'waiting' })]);
    const chat = (id: string) => useChats.getState().findChat(id);
    expect([chat('f1')?.status, chat('f2')?.status]).toEqual(['resting', 'resting']);
    expect(chat('f1')?.handoffAsk).toEqual(ask);
  });

  it('looks a saved title up once a chat, and again for a chat deleted and back', async () => {
    const saved = vi.mocked(sessionsIpc.savedTitle);
    const chat = chatOf('t1', { sessionId: 'aaaa-1', nameSource: 'prompt' });
    const before = saved.mock.calls.length;
    await tickOver([chat]);
    await tickOver([chat]);
    expect(saved.mock.calls.length).toBe(before + 1);
    // Deleted: what was kept of it goes at the next tick.
    await tickOver([chatOf('other')]);
    await tickOver([chat]);
    expect(saved.mock.calls.length).toBe(before + 2);
  });

  it('forgets the saved-title lookups when no chat is listed at all', async () => {
    const saved = vi.mocked(sessionsIpc.savedTitle);
    const chat = chatOf('t2', { sessionId: 'aaaa-2', nameSource: 'prompt' });
    const before = saved.mock.calls.length;
    await tickOver([chat]);
    expect(saved.mock.calls.length).toBe(before + 1);
    // The only folder removed: a round with nothing to ask the core still
    // lets the lookups of the chats gone go, before it stops.
    useChats.setState({ folders: [] });
    const asked = vi.mocked(sessionsIpc.sessionsMeta).mock.calls.length;
    const empty = renderHook(() => useSessionWatch());
    await new Promise(r => setTimeout(r, 0));
    empty.unmount();
    expect(vi.mocked(sessionsIpc.sessionsMeta).mock.calls.length).toBe(asked);
    // The chat restored: looked up again.
    await tickOver([chat]);
    expect(saved.mock.calls.length).toBe(before + 2);
  });
});
