import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { AccountInfo, AccountLimitsDto, CodexLimitsDto } from '../../ipc/accounts';

vi.mock('../../ipc/accounts', async importOriginal => ({
  ...(await importOriginal<typeof import('../../ipc/accounts')>()),
  listAccounts: vi.fn(),
  accountLimits: vi.fn(),
  codexLimits: vi.fn(),
  createAccount: vi.fn(),
  deleteAccount: vi.fn(),
  renameAccount: vi.fn()
}));
vi.mock('../../ipc/agents', async importOriginal => ({
  ...(await importOriginal<typeof import('../../ipc/agents')>()),
  agentBlockedAccounts: vi.fn(() => Promise.resolve([]))
}));

// A rename reaches the chats store, which reaches the terminals; xterm cannot run in jsdom.
vi.mock('../panes/terminals', () => ({
  startSession: vi.fn(),
  focusTerminal: vi.fn(),
  forget: vi.fn(),
  forgetAll: vi.fn()
}));

const work: AccountInfo = { provider: 'claude', name: 'work', path: '/acc/claude/work' };
const gpt: AccountInfo = { provider: 'codex', name: 'gpt', path: '/acc/codex/gpt' };

const claudeLimits = (over: Partial<AccountLimitsDto> = {}): AccountLimitsDto => ({
  signedIn: true,
  email: 'me@example.com',
  plan: 'max',
  haveUsage: true,
  h5: 0.25,
  week: 0.5,
  resetH5: null,
  resetWeek: null,
  scoped: [],
  source: 'network',
  fetchedAtMs: Date.now(),
  stale: false,
  refreshError: null,
  rateLimited: null,
  ...over
});

const codexLimits = (over: Partial<CodexLimitsDto> = {}): CodexLimitsDto => ({
  signedIn: true,
  email: null,
  plan: 'pro',
  haveUsage: true,
  windows: [],
  source: 'network',
  fetchedAtMs: Date.now(),
  stale: false,
  rateLimited: null,
  ...over
});

/**
 * The store keeps its polling and backoff in module state, so each test loads
 * it afresh, together with the mocked commands it talks to.
 */
async function load() {
  vi.resetModules();
  const ipc = vi.mocked(await import('../../ipc/accounts'));
  const { useAccounts } = await import('./accounts.store');
  const { useChats } = await import('../chats/chats.store');
  const { useNewChat } = await import('../new-chat/newchat.store');
  return { ipc, accounts: useAccounts, useChats, useNewChat, state: () => useAccounts.getState() };
}

beforeEach(() => {
  vi.useFakeTimers();
  // No jitter: the waits between rounds are then exact.
  vi.spyOn(Math, 'random').mockReturnValue(0);
});

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

describe('accounts store: a refresh round', () => {
  it('lists the accounts and marks the list loaded', async () => {
    const { ipc, state } = await load();
    ipc.listAccounts.mockResolvedValue([work]);
    ipc.accountLimits.mockResolvedValue(claudeLimits());
    expect(state().loaded).toBe(false);
    await state().refresh();
    expect(state().loaded).toBe(true);
    expect(state().accounts.map(a => a.name)).toEqual(['work']);
  });

  it('shows a Claude account’s 5-hour and weekly use, its plan and email', async () => {
    const { ipc, state } = await load();
    ipc.listAccounts.mockResolvedValue([work]);
    ipc.accountLimits.mockResolvedValue(claudeLimits());
    await state().refresh();
    const [a] = state().accounts;
    expect(a).toMatchObject({ plan: 'max', email: 'me@example.com', signedIn: true, sync: 'ready' });
    expect(a!.limits.map(l => [l.short, l.used])).toEqual([
      ['5H', 0.25],
      ['W', 0.5]
    ]);
  });

  it('keys a model-scoped weekly limit by the model’s initial', async () => {
    const { ipc, state } = await load();
    ipc.listAccounts.mockResolvedValue([work]);
    ipc.accountLimits.mockResolvedValue(claudeLimits({ scoped: [{ label: 'Fable', used: 0.1, resetsAt: null }] }));
    await state().refresh();
    expect(state().accounts[0]!.limits.at(-1)).toMatchObject({ label: 'Fable · week', short: 'F' });
  });

  it('keys Codex’s windows, its extra limits by initial and window', async () => {
    const { ipc, state } = await load();
    ipc.listAccounts.mockResolvedValue([gpt]);
    const win = (id: string, label: string) => ({ id, label, used: 0, windowMinutes: null, resetsAt: null });
    ipc.codexLimits.mockResolvedValue(
      codexLimits({ windows: [win('p', '5 hours'), win('s', 'week'), win('g', 'GPT-5 · week')] })
    );
    await state().refresh();
    expect(state().accounts[0]!.limits.map(l => l.short)).toEqual(['5H', 'W', 'GW']);
  });

  it('marks an account whose limits could not be fetched as failed', async () => {
    const { ipc, state } = await load();
    ipc.listAccounts.mockResolvedValue([work]);
    ipc.accountLimits.mockRejectedValue(new Error('offline'));
    await state().refresh();
    expect(state().accounts[0]!.sync).toBe('error');
  });

  it('keeps the last numbers on the bars through a round that brought none', async () => {
    const { ipc, state } = await load();
    ipc.listAccounts.mockResolvedValue([work]);
    ipc.accountLimits.mockResolvedValue(claudeLimits());
    await state().refresh();
    ipc.accountLimits.mockResolvedValue(claudeLimits({ haveUsage: false, rateLimited: 30 }));
    await state().refresh();
    const [a] = state().accounts;
    expect(a!.sync).toBe('throttled');
    expect(a!.haveUsage).toBe(true);
    expect(a!.limits.map(l => l.used)).toEqual([0.25, 0.5]);
  });

  it('marks numbers off the CLI’s cache as stale', async () => {
    const { ipc, state } = await load();
    ipc.listAccounts.mockResolvedValue([work]);
    ipc.accountLimits.mockResolvedValue(claudeLimits({ stale: true }));
    await state().refresh();
    expect(state().accounts[0]!.sync).toBe('stale');
  });

  it('keeps the same account list object when nothing changed, so nothing re-renders', async () => {
    const { ipc, state } = await load();
    ipc.listAccounts.mockResolvedValue([work]);
    ipc.accountLimits.mockResolvedValue(claudeLimits({ fetchedAtMs: 1 }));
    await state().refresh();
    const before = state().accounts;
    await state().refresh();
    expect(state().accounts).toBe(before);
  });

  it('asks the accounts one at a time, with a pause between them', async () => {
    const { ipc, state } = await load();
    ipc.listAccounts.mockResolvedValue([work, gpt]);
    ipc.accountLimits.mockResolvedValue(claudeLimits());
    ipc.codexLimits.mockResolvedValue(codexLimits());
    const round = state().refresh();
    await vi.advanceTimersByTimeAsync(1999);
    expect(ipc.codexLimits).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(1);
    await round;
    expect(ipc.codexLimits).toHaveBeenCalledOnce();
  });
});

describe('accounts store: polling', () => {
  /** Lets the round in flight run to its end without moving the clock. */
  const settle = () => vi.advanceTimersByTimeAsync(0);

  it('polls again a minute after a healthy round', async () => {
    const { ipc, state } = await load();
    ipc.listAccounts.mockResolvedValue([work]);
    ipc.accountLimits.mockResolvedValue(claudeLimits());
    state().startPolling();
    await settle();
    expect(ipc.listAccounts).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(59_999);
    expect(ipc.listAccounts).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(1);
    expect(ipc.listAccounts).toHaveBeenCalledTimes(2);
    state().stopPolling();
  });

  it('backs off 30s, then 60s, while there is nothing to show', async () => {
    const { ipc, state } = await load();
    ipc.listAccounts.mockResolvedValue([work]);
    ipc.accountLimits.mockResolvedValue(null);
    state().startPolling();
    await settle();
    await vi.advanceTimersByTimeAsync(30_000);
    expect(ipc.listAccounts).toHaveBeenCalledTimes(2);
    await vi.advanceTimersByTimeAsync(59_999);
    expect(ipc.listAccounts).toHaveBeenCalledTimes(2);
    await vi.advanceTimersByTimeAsync(1);
    expect(ipc.listAccounts).toHaveBeenCalledTimes(3);
    state().stopPolling();
  });

  it('sits out three minutes after a 429, even with numbers on the bars', async () => {
    const { ipc, state } = await load();
    ipc.listAccounts.mockResolvedValue([work]);
    ipc.accountLimits.mockResolvedValueOnce(claudeLimits());
    ipc.accountLimits.mockResolvedValue(claudeLimits({ haveUsage: false, rateLimited: 0 }));
    await state().refresh();
    state().startPolling();
    await settle();
    expect(state().accounts[0]!.sync).toBe('throttled');
    await vi.advanceTimersByTimeAsync(179_999);
    expect(ipc.listAccounts).toHaveBeenCalledTimes(2);
    await vi.advanceTimersByTimeAsync(1);
    expect(ipc.listAccounts).toHaveBeenCalledTimes(3);
    state().stopPolling();
  });

  it('keeps polling after the account list itself fails, e.g. while the core restarts', async () => {
    const { ipc, state } = await load();
    ipc.listAccounts.mockRejectedValueOnce(new Error('the core is not running'));
    ipc.listAccounts.mockResolvedValue([work]);
    ipc.accountLimits.mockResolvedValue(claudeLimits());
    state().startPolling();
    await settle();
    await vi.advanceTimersByTimeAsync(30_000);
    expect(ipc.listAccounts).toHaveBeenCalledTimes(2);
    expect(state().loaded).toBe(true);
    state().stopPolling();
  });

  it('stops for good on stopPolling', async () => {
    const { ipc, state } = await load();
    ipc.listAccounts.mockResolvedValue([work]);
    ipc.accountLimits.mockResolvedValue(claudeLimits());
    state().startPolling();
    await settle();
    state().stopPolling();
    await vi.advanceTimersByTimeAsync(10 * 60_000);
    expect(ipc.listAccounts).toHaveBeenCalledTimes(1);
  });

  it('starts only one poller however often it is asked', async () => {
    const { ipc, state } = await load();
    ipc.listAccounts.mockResolvedValue([work]);
    ipc.accountLimits.mockResolvedValue(claudeLimits());
    state().startPolling();
    state().startPolling();
    await settle();
    expect(ipc.listAccounts).toHaveBeenCalledTimes(1);
    state().stopPolling();
  });
});

describe('accounts store: the refresh button', () => {
  it('asks past the cache and shows the answer', async () => {
    const { ipc, state } = await load();
    ipc.listAccounts.mockResolvedValue([work]);
    ipc.accountLimits.mockResolvedValue(null);
    await state().refresh();
    ipc.accountLimits.mockResolvedValue(claudeLimits({ h5: 0.9 }));
    await state().refreshAccount(state().accounts[0]!);
    expect(ipc.accountLimits).toHaveBeenLastCalledWith(work.path, true);
    expect(state().accounts[0]!.limits[0]!.used).toBe(0.9);
    expect(state().refreshing).toEqual([]);
  });

  it('ignores a second click while the first is on its way', async () => {
    const { ipc, state } = await load();
    ipc.listAccounts.mockResolvedValue([work]);
    ipc.accountLimits.mockResolvedValue(claudeLimits());
    await state().refresh();
    ipc.accountLimits.mockClear();
    const first = state().refreshAccount(state().accounts[0]!);
    expect(state().refreshing).toEqual([work.path]);
    await state().refreshAccount(state().accounts[0]!);
    await first;
    expect(ipc.accountLimits).toHaveBeenCalledOnce();
  });

  it('is not overwritten by a round that asked before the click answered', async () => {
    const { ipc, state } = await load();
    ipc.listAccounts.mockResolvedValue([work]);
    ipc.accountLimits.mockResolvedValue(claudeLimits({ h5: 0.1 }));
    await state().refresh();

    // A round asks with the old numbers still coming; the click answers first.
    let answerRound: (v: AccountLimitsDto) => void = () => {};
    ipc.accountLimits.mockImplementationOnce(() => new Promise(r => (answerRound = r)));
    const round = state().refresh();
    await vi.advanceTimersByTimeAsync(0);
    await vi.advanceTimersByTimeAsync(1000);
    ipc.accountLimits.mockResolvedValueOnce(claudeLimits({ h5: 0.7 }));
    await state().refreshAccount(state().accounts[0]!);
    answerRound(claudeLimits({ h5: 0.2 }));
    await round;
    expect(state().accounts[0]!.limits[0]!.used).toBe(0.7);
  });
});

describe('accounts store: managing accounts', () => {
  it('ignores a blank name', async () => {
    const { ipc, state } = await load();
    await state().add('claude', '   ');
    expect(ipc.createAccount).not.toHaveBeenCalled();
  });

  it('creates a trimmed name and opens a first login for it', async () => {
    const { ipc, state } = await load();
    ipc.createAccount.mockResolvedValue(work);
    ipc.listAccounts.mockResolvedValue([work]);
    ipc.accountLimits.mockResolvedValue(claudeLimits());
    state().setAdding(true);
    await state().add('claude', '  work ');
    expect(ipc.createAccount).toHaveBeenCalledWith('claude', 'work');
    expect(state().adding).toBe(false);
    expect(state().loginFor?.path).toBe(work.path);
  });

  it('says why an account could not be made', async () => {
    const { ipc, state } = await load();
    ipc.createAccount.mockRejectedValue('name taken');
    await state().add('claude', 'work');
    expect(state().error).toBe('name taken');
  });

  it('carries a rename over to the chats on it and to the last account used', async () => {
    const { ipc, state, useChats, useNewChat } = await load();
    ipc.renameAccount.mockResolvedValue({ ...work, name: 'job' });
    ipc.listAccounts.mockResolvedValue([{ ...work, name: 'job' }]);
    ipc.accountLimits.mockResolvedValue(claudeLimits());
    useChats.getState().addChat(
      '/repo',
      {
        id: 'c1',
        name: 'chat',
        status: 'resting',
        context: 0,
        account: 'work',
        group: 0,
        worktree: false,
        provider: 'claude',
        model: 'Opus',
        effort: 'high',
        perm: 'Bypass'
      },
      true
    );
    useNewChat.getState().remember('/repo', { provider: 'claude', name: 'work' });
    await state().rename('claude', 'work', ' job ');
    expect(useChats.getState().findChat('c1')?.account).toBe('job');
    expect(useNewChat.getState().lastAccount).toEqual({ provider: 'claude', name: 'job' });
  });
});
