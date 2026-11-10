import { act, renderHook } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import * as agents from '../../ipc/agents';
import type { BackgroundHelperDto, PromotedDto } from '../../ipc/agents';
import type { Account, Chat } from '../../shared/types';
import { useAccounts } from '../accounts/accounts.store';
import { useChats } from '../chats/chats.store';
import { useCore } from '../core/core.store';
import { currentSlots, usePanes } from '../panes/panes.store';
import { useBackground } from './background.store';
import { openAsChat, useAgentBridge } from './bridge';

/** The core's events, as the bridge subscribed to them: fire one with `emit`. */
const events = vi.hoisted(() => {
  const handlers = new Map<string, (payload: unknown) => void>();
  const on = (name: string) => (cb: (payload: unknown) => void) => {
    handlers.set(name, cb);
    return Promise.resolve(() => {
      handlers.delete(name);
    });
  };
  return { handlers, on };
});

vi.mock('../../ipc/events', () => ({
  onBackgroundHelpers: events.on('agent://background'),
  onCoreState: events.on('core://state')
}));

// The settings and trust a chat made from a helper reads on the way: no
// settings files say anything, and every folder is trusted.
vi.mock('../../ipc/defaults', async importOriginal => ({
  ...(await importOriginal<typeof import('../../ipc/defaults')>()),
  claudeDefaults: vi.fn(() => Promise.resolve({ model: null, effort: null, permissionMode: null })),
  codexDefaults: vi.fn(() => Promise.resolve({ model: null, effort: null, approval: null, sandbox: null })),
  codexModels: vi.fn(() => Promise.resolve([]))
}));
vi.mock('../../ipc/trust', async importOriginal => ({
  ...(await importOriginal<typeof import('../../ipc/trust')>()),
  claudeFolderTrusted: vi.fn(() => Promise.resolve(true)),
  codexFolderTrusted: vi.fn(() => Promise.resolve(true)),
  codexHooksStart: vi.fn(() => Promise.resolve({ default: false, kept: null }))
}));
vi.mock('../../ipc/agents', async importOriginal => ({
  ...(await importOriginal<typeof import('../../ipc/agents')>()),
  backgroundHelpers: vi.fn(() => Promise.resolve([])),
  backgroundPromote: vi.fn(),
  backgroundPromoted: vi.fn(() => Promise.resolve(true)),
  chatNames: vi.fn(() => Promise.resolve()),
  accountUsage: vi.fn(() => Promise.resolve())
}));

// xterm cannot run in jsdom.
vi.mock('../panes/terminals', () => ({
  startSession: vi.fn(() => Promise.resolve('')),
  focusTerminal: vi.fn(),
  forget: vi.fn(),
  forgetAll: vi.fn()
}));

const mocked = vi.mocked(agents);

const account = (provider: Account['provider'], name: string): Account => ({
  provider,
  name,
  path: `/accounts/${provider}/${name}`,
  plan: 'max',
  email: null,
  signedIn: true,
  haveUsage: false,
  limits: [],
  usageAge: null,
  fetchedAt: null,
  sync: 'ready',
  refreshError: null
});

const parent: Chat = {
  id: 'parent',
  name: 'the caller',
  status: 'working',
  context: 0,
  account: 'work',
  group: 0,
  worktree: false,
  provider: 'claude',
  model: 'Opus',
  effort: 'high',
  perm: 'Bypass'
};

const row = (over: Partial<BackgroundHelperDto> = {}): BackgroundHelperDto => ({
  id: 'bg1',
  parent: 'parent',
  name: 'background job',
  provider: 'claude',
  state: 'idle',
  account: 'work',
  model: 'haiku',
  effort: 'low',
  worktree: false,
  startedMs: 0,
  turnStartedMs: null,
  lastActivityMs: null,
  task: null,
  ...over
});

const emit = (name: string, payload: unknown) => act(() => events.handlers.get(name)!(payload));

async function mountBridge() {
  const view = renderHook(() => useAgentBridge());
  await vi.waitFor(() => expect(events.handlers.has('agent://background')).toBe(true));
  return view;
}

beforeEach(() => {
  vi.clearAllMocks();
  mocked.backgroundPromote.mockReset();
  mocked.backgroundHelpers.mockResolvedValue([]);
  vi.spyOn(console, 'warn').mockImplementation(() => {});
  useAccounts.setState({ accounts: [account('claude', 'work'), account('codex', 'gpt')], loaded: true });
  useChats.getState().addChat('/repo', parent, true);
});

describe('agent bridge: the helpers', () => {
  it('keeps the list the core sends, and settles an opening a page gone before it left', async () => {
    // Its row made, then that page went: the core is told it is the person's now.
    useChats.getState().addChat('/repo', { ...parent, id: 'bg-made' }, false);
    mocked.backgroundHelpers.mockResolvedValue([
      row({ id: 'bg-made', promoting: true }),
      row({ id: 'bg-cut', promoting: true })
    ]);
    await mountBridge();
    await vi.waitFor(() => expect(useBackground.getState().helpers).toHaveLength(2));
    expect(mocked.backgroundPromoted).toHaveBeenCalledWith('bg-made', null);
    expect(mocked.backgroundPromoted).toHaveBeenCalledWith(
      'bg-cut',
      'the window that was opening it as a chat went away'
    );
    emit('agent://background', [row()]);
    expect(useBackground.getState().helpers.map(h => h.id)).toEqual(['bg1']);
  });

  it('stops listening once unmounted', async () => {
    const view = await mountBridge();
    view.unmount();
    expect(events.handlers.has('agent://background')).toBe(false);
  });
});

describe('agent bridge: what the core is told', () => {
  it('sends the rows’ names and groups, and again only when they change', async () => {
    await mountBridge();
    expect(mocked.chatNames).toHaveBeenLastCalledWith({ parent: 'the caller' }, { parent: 0 });
    act(() => useChats.getState().setActive('parent'));
    expect(mocked.chatNames).toHaveBeenCalledOnce();
    act(() => useChats.getState().renameChat('parent', 'renamed'));
    expect(mocked.chatNames).toHaveBeenLastCalledWith({ parent: 'renamed' }, { parent: 0 });
  });

  it('sends the same names again at the next change of the list after a send that failed', async () => {
    mocked.chatNames.mockRejectedValueOnce(new Error('the core is not running'));
    await mountBridge();
    // Nothing the core is told changes here; only the failed send makes it go again.
    act(() => useChats.getState().setActive('parent'));
    expect(mocked.chatNames).toHaveBeenCalledTimes(2);
  });

  it('sends the names again to a core that has just come up', async () => {
    await mountBridge();
    act(() => useCore.setState({ link: { state: 'up', core: { pid: 7 } as never, error: null } }));
    expect(mocked.chatNames).toHaveBeenCalledTimes(2);
  });
});

describe('agent bridge: opening a background helper as a chat', () => {
  const helper = (over: Partial<PromotedDto> = {}): PromotedDto => ({
    id: 'bg1',
    provider: 'claude',
    accountPath: '/accounts/claude/work',
    folder: '/repo',
    worktreePath: null,
    sessionId: 'sess-1',
    name: 'background job',
    settings: { model: 'haiku' },
    tools: false,
    ...over
  });

  it('makes a top-level row of the person’s own, keeping its id and session, seated, and hands it over', async () => {
    mocked.backgroundPromote.mockResolvedValue(helper());
    await openAsChat('bg1');
    const chat = useChats.getState().findChat('bg1');
    expect(chat).toMatchObject({ model: 'Haiku', sessionId: 'sess-1', name: 'background job' });
    expect(chat).not.toHaveProperty('parentId');
    expect(currentSlots(usePanes.getState())).toContain('bg1');
    expect(mocked.backgroundPromoted).toHaveBeenCalledWith('bg1', null);
  });

  it('opens a Codex helper the same way, its approval and sandbox as it ran', async () => {
    mocked.backgroundPromote.mockResolvedValue(
      helper({
        id: 'bg2',
        provider: 'codex',
        accountPath: '/accounts/codex/gpt',
        settings: { model: 'gpt-5', effort: 'low', approval: 'never', sandbox: 'read-only' }
      })
    );
    await openAsChat('bg2');
    expect(useChats.getState().findChat('bg2')).toMatchObject({
      provider: 'codex',
      model: 'gpt-5',
      approval: 'never',
      sandbox: 'read-only'
    });
    expect(mocked.backgroundPromoted).toHaveBeenCalledWith('bg2', null);
  });

  it('finds the account whatever the path’s slashes and case', async () => {
    mocked.backgroundPromote.mockResolvedValue(helper({ accountPath: '\\ACCOUNTS\\claude\\work\\' }));
    await openAsChat('bg1');
    expect(useChats.getState().findChat('bg1')).not.toBeNull();
  });

  it('tells the core why, and leaves the helper its agent’s, when no row can be made', async () => {
    mocked.backgroundPromote.mockResolvedValue(helper({ accountPath: '/accounts/claude/other' }));
    await expect(openAsChat('bg1')).rejects.toThrow('no claude account at /accounts/claude/other');
    expect(mocked.backgroundPromoted).toHaveBeenCalledWith('bg1', 'no claude account at /accounts/claude/other');
    expect(useChats.getState().findChat('bg1')).toBeNull();
  });

  it('makes one row for two clicks in a row', async () => {
    mocked.backgroundPromote.mockResolvedValue(helper());
    await Promise.all([openAsChat('bg1'), openAsChat('bg1')]);
    expect(mocked.backgroundPromote).toHaveBeenCalledOnce();
  });

  it('tells the core again when it could not be told, and seats the row only once it has let go', async () => {
    mocked.backgroundPromote.mockResolvedValue(helper());
    mocked.backgroundPromoted.mockRejectedValueOnce(new Error('the core is not running'));
    const opened = openAsChat('bg1');
    await vi.waitFor(() => expect(mocked.backgroundPromoted).toHaveBeenCalledOnce());
    expect(currentSlots(usePanes.getState())).not.toContain('bg1');
    await opened;
    expect(mocked.backgroundPromoted).toHaveBeenCalledTimes(2);
    expect(currentSlots(usePanes.getState())).toContain('bg1');
  });

  it('lets go of the row when the core gave the opening up, the helper its agent’s again', async () => {
    mocked.backgroundPromote.mockResolvedValue(helper());
    mocked.backgroundPromoted.mockResolvedValueOnce(false);
    await openAsChat('bg1');
    expect(useChats.getState().findChat('bg1')).toBeNull();
    expect(currentSlots(usePanes.getState())).not.toContain('bg1');
  });

  it('lets go of a row a page gone before it made, for an opening the core gave up since', async () => {
    useChats.getState().addChat('/repo', { ...parent, id: 'bg-lapsed' }, false);
    mocked.backgroundHelpers.mockResolvedValue([row({ id: 'bg-lapsed' })]);
    mocked.backgroundPromoted.mockResolvedValueOnce(false);
    await mountBridge();
    await vi.waitFor(() => expect(useChats.getState().findChat('bg-lapsed')).toBeNull());
    expect(mocked.backgroundPromoted).toHaveBeenCalledWith('bg-lapsed', null);
  });
});
