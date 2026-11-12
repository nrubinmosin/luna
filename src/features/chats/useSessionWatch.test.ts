import { renderHook, waitFor } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import * as handoffIpc from '../../ipc/handoff';
import type { Account, Chat } from '../../shared/types';
import { useAccounts } from '../accounts/accounts.store';
import { useChats } from './chats.store';
import { mapStatus, useSessionWatch } from './useSessionWatch';

vi.mock('../../ipc/sessions', async importOriginal => ({
  ...(await importOriginal<typeof import('../../ipc/sessions')>()),
  // The CLI not running: nothing of it to say.
  sessionMeta: vi.fn(() => Promise.resolve(null)),
  savedTitle: vi.fn(() => Promise.resolve(null))
}));
vi.mock('../../ipc/handoff', async importOriginal => ({
  ...(await importOriginal<typeof import('../../ipc/handoff')>()),
  handoffAsk: vi.fn(() => Promise.resolve(null))
}));

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
    const account = { provider: 'claude', name: 'work', path: '/accounts/claude/work' } as Account;
    const chat = {
      id: 'w1',
      name: 'campaign',
      provider: 'claude',
      account: 'work',
      status: 'resting',
      tools: true,
      handoffAsk: { kind: 'manual', at: 1000 }
    } as Chat;
    useAccounts.setState({ accounts: [account], loaded: true });
    useChats.setState({ folders: [{ id: 'f', path: '/repo', open: true, chats: [chat] }] });
    const { unmount } = renderHook(() => useSessionWatch());
    // The core lapsed it while the CLI was down: the chip and the menu's
    // refusal go with it.
    await waitFor(() => expect(useChats.getState().findChat('w1')?.handoffAsk ?? null).toBeNull());
    expect(handoffIpc.handoffAsk).toHaveBeenCalledWith('w1');
    unmount();
  });
});
