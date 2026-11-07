import { act, renderHook } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import * as ipc from '../../ipc/commands';
import type { AgentSpawnRequest, CodexModelDto, PromotedDto } from '../../ipc/commands';
import type { Account, Chat } from '../../shared/types';
import { useAccounts } from '../accounts/accounts.store';
import { useChats } from '../chats/chats.store';
import { useCore } from '../core/core.store';
import { currentSlots, usePanes } from '../panes/panes.store';
import { openAsChat, useAgentBridge } from './bridge';

/** The core's events, as the bridge subscribed to them: fire one with `emit`. */
const events = vi.hoisted(() => {
  const handlers = new Map<string, (payload: unknown) => void>();
  const off = { count: 0 };
  const on = (name: string) => (cb: (payload: unknown) => void) => {
    handlers.set(name, cb);
    return Promise.resolve(() => {
      off.count++;
      handlers.delete(name);
    });
  };
  return { handlers, off, on };
});

vi.mock('../../ipc/events', () => ({
  onAgentSpawn: events.on('agent://spawn'),
  onAgentDeleted: events.on('agent://deleted'),
  onAgentReparent: events.on('agent://reparent'),
  onBackgroundHelpers: events.on('agent://background'),
  onCoreState: events.on('core://state')
}));

// The commands a spawn sends to the core, and the settings and trust it reads
// on the way: no settings files say anything, and every folder is trusted.
vi.mock('../../ipc/commands', async importOriginal => ({
  ...(await importOriginal<typeof import('../../ipc/commands')>()),
  claudeDefaults: vi.fn(() => Promise.resolve({ model: null, effort: null, permissionMode: null })),
  codexDefaults: vi.fn(() => Promise.resolve({ model: null, effort: null, approval: null, sandbox: null })),
  claudeFolderTrusted: vi.fn(() => Promise.resolve(true)),
  codexFolderTrusted: vi.fn(() => Promise.resolve(true)),
  codexHooksStart: vi.fn(() => Promise.resolve({ default: false, kept: null })),
  agentSpawned: vi.fn(() => Promise.resolve()),
  agentSpawns: vi.fn(() => Promise.resolve([])),
  agentResumes: vi.fn(() => Promise.resolve([])),
  backgroundHelpers: vi.fn(() => Promise.resolve([])),
  backgroundPromote: vi.fn(),
  backgroundPromoted: vi.fn(() => Promise.resolve()),
  chatNames: vi.fn(() => Promise.resolve()),
  accountUsage: vi.fn(() => Promise.resolve()),
  codexModels: vi.fn(() => Promise.resolve([])),
  deleteSession: vi.fn(() => Promise.resolve()),
  ensureClaudeSession: vi.fn(() => Promise.resolve('session')),
  ensureCodexSession: vi.fn(() => Promise.resolve('session'))
}));

// xterm cannot run in jsdom.
vi.mock('../panes/terminals', () => ({
  startSession: vi.fn(() => Promise.resolve('')),
  focusTerminal: vi.fn(),
  forget: vi.fn(),
  forgetAll: vi.fn()
}));

const mocked = vi.mocked(ipc);

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

// The bridge takes each request id once for the life of the page, so every
// test asks with ids of its own.
let nextRequest = 100;
const request = (over: Partial<AgentSpawnRequest> = {}): AgentSpawnRequest => ({
  requestId: nextRequest++,
  parentId: 'parent',
  provider: 'claude',
  account: 'work',
  folder: '',
  prompt: 'do the thing',
  hooksUnasked: false,
  ...over
});
const rowOf = (req: AgentSpawnRequest) => `c${req.requestId.toString(36)}`;

const emit = (name: string, payload: unknown) => act(() => events.handlers.get(name)!(payload));

/** The bridge's answer to a spawn request, once it has given one. */
async function answerTo(req: AgentSpawnRequest) {
  const answer = () => mocked.agentSpawned.mock.calls.find(c => c[0] === req.requestId);
  await vi.waitFor(() => expect(answer()).toBeDefined());
  const [, chatId, , error] = answer()!;
  return { chatId, error };
}

async function mountBridge() {
  const view = renderHook(() => useAgentBridge());
  await vi.waitFor(() => expect(events.handlers.has('agent://spawn')).toBe(true));
  return view;
}

beforeEach(() => {
  vi.clearAllMocks();
  // A refused spawn is logged as well as answered; the answer is what is tested.
  vi.spyOn(console, 'warn').mockImplementation(() => {});
  events.off.count = 0;
  useAccounts.setState({ accounts: [account('claude', 'work'), account('codex', 'gpt')], loaded: true });
  useChats.getState().addChat('/repo', parent, true);
});

describe('agent bridge: a spawn request', () => {
  it('becomes a row under its caller, a running session, and an answer naming the row', async () => {
    await mountBridge();
    const req = request({ model: 'sonnet', effort: 'low', name: 'helper' });
    emit('agent://spawn', req);
    const { chatId, error } = await answerTo(req);
    expect(error).toBeNull();
    expect(chatId).toBe(rowOf(req));
    expect(useChats.getState().findChat(rowOf(req))).toMatchObject({ parentId: 'parent', name: 'helper' });
    expect(mocked.ensureClaudeSession).toHaveBeenCalledWith(
      expect.objectContaining({
        chatId: rowOf(req),
        folder: '/repo',
        model: 'Sonnet',
        effort: 'low',
        prompt: 'do the thing',
        spawnRequest: req.requestId
      })
    );
  });

  it('runs in the folder it names over its caller’s', async () => {
    await mountBridge();
    const req = request({ folder: '/elsewhere' });
    emit('agent://spawn', req);
    await answerTo(req);
    expect(useChats.getState().folderOf(rowOf(req))?.path).toBe('/elsewhere');
  });

  it('does not seat a helper with a caller in a pane: the caller is what is being watched', async () => {
    await mountBridge();
    const req = request();
    emit('agent://spawn', req);
    await answerTo(req);
    expect(currentSlots(usePanes.getState())).not.toContain(rowOf(req));
  });

  it('seats a top-level chat once its session runs', async () => {
    await mountBridge();
    const req = request({ parentId: 'gone', folder: '/repo' });
    emit('agent://spawn', req);
    await answerTo(req);
    expect(currentSlots(usePanes.getState())).toContain(rowOf(req));
  });

  it('is taken up once, though it arrives both as an event and when asked for', async () => {
    const req = request();
    mocked.agentSpawns.mockResolvedValue([req]);
    await mountBridge();
    emit('agent://spawn', req);
    await answerTo(req);
    expect(mocked.ensureClaudeSession).toHaveBeenCalledOnce();
  });

  it('asks again for waiting spawns when a new core comes up', async () => {
    await mountBridge();
    await vi.waitFor(() => expect(mocked.agentSpawns).toHaveBeenCalledOnce());
    act(() => useCore.setState({ link: { state: 'up', core: { pid: 42 } as never, error: null } }));
    expect(mocked.agentSpawns).toHaveBeenCalledTimes(2);
  });

  it('waits for the account list before looking the account up', async () => {
    useAccounts.setState({ accounts: [], loaded: false });
    await mountBridge();
    const req = request();
    emit('agent://spawn', req);
    await Promise.resolve();
    expect(mocked.agentSpawned).not.toHaveBeenCalled();
    act(() => useAccounts.setState({ accounts: [account('claude', 'work')], loaded: true }));
    expect((await answerTo(req)).error).toBeNull();
  });
});

describe('agent bridge: refusing a spawn', () => {
  /** Sends the request and returns the error it was answered with. */
  async function refusal(over: Partial<AgentSpawnRequest>) {
    await mountBridge();
    const req = request(over);
    emit('agent://spawn', req);
    const { chatId, error } = await answerTo(req);
    expect(chatId).toBeNull();
    // Refused before anything was made: no row, no session.
    expect(useChats.getState().findChat(rowOf(req))).toBeNull();
    expect(mocked.ensureClaudeSession).not.toHaveBeenCalled();
    expect(mocked.ensureCodexSession).not.toHaveBeenCalled();
    return error;
  }

  it('names an account that does not exist', async () => {
    expect(await refusal({ account: 'nobody' })).toBe('no claude account named nobody');
  });

  it('names a Claude model it does not know, with the ones it does', async () => {
    expect(await refusal({ model: 'gpt-9' })).toMatch(/^unknown Claude model "gpt-9"; use .*opus/);
  });

  it('turns down Codex’s settings on a Claude chat', async () => {
    expect(await refusal({ sandbox: 'read-only' })).toMatch(/Codex settings/);
  });

  it('turns down Claude’s permission mode on a Codex chat', async () => {
    expect(await refusal({ provider: 'codex', account: 'gpt', permissionMode: 'plan' })).toMatch(
      /permissionMode is a Claude setting/
    );
  });

  it('turns down a sandbox other than auto-review’s own', async () => {
    expect(
      await refusal({ provider: 'codex', account: 'gpt', approval: 'auto-review', sandbox: 'danger-full-access' })
    ).toMatch(/^auto-review runs in/);
  });

  it('turns down a Codex model the account does not list', async () => {
    mocked.codexModels.mockResolvedValue([model('gpt-5', ['low', 'high'])]);
    expect(await refusal({ provider: 'codex', account: 'gpt', model: 'gpt-4' })).toBe(
      'Codex on gpt does not list model "gpt-4"; it has gpt-5'
    );
  });

  it('turns down an effort the Codex model does not take', async () => {
    mocked.codexModels.mockResolvedValue([model('gpt-5', ['low', 'high'])]);
    expect(await refusal({ provider: 'codex', account: 'gpt', model: 'gpt-5', effort: 'max' })).toBe(
      'gpt-5 takes effort low, high, not "max"'
    );
  });

  it('drops the row and its session when the session cannot start', async () => {
    mocked.ensureClaudeSession.mockRejectedValueOnce(new Error('cli missing'));
    await mountBridge();
    const req = request();
    emit('agent://spawn', req);
    expect((await answerTo(req)).error).toBe('cli missing');
    expect(useChats.getState().findChat(rowOf(req))).toBeNull();
    expect(mocked.deleteSession).toHaveBeenCalledWith(rowOf(req), '/repo', null, true);
  });
});

const model = (slug: string, efforts: string[], defaultEffort: string | null = null): CodexModelDto => ({
  slug,
  displayName: slug,
  defaultEffort,
  efforts,
  effortHints: {}
});

describe('agent bridge: Codex settings', () => {
  it('gives a model asked for its own default effort', async () => {
    mocked.codexModels.mockResolvedValue([model('gpt-5', ['low', 'medium', 'high'], 'medium')]);
    await mountBridge();
    const req = request({ provider: 'codex', account: 'gpt', model: 'gpt-5' });
    emit('agent://spawn', req);
    expect((await answerTo(req)).error).toBeNull();
    expect(mocked.ensureCodexSession).toHaveBeenCalledWith(
      expect.objectContaining({ model: 'gpt-5', effort: 'medium' })
    );
  });

  it('puts auto-review in its own sandbox when none is named', async () => {
    await mountBridge();
    const req = request({ provider: 'codex', account: 'gpt', approval: 'auto-review' });
    emit('agent://spawn', req);
    expect((await answerTo(req)).error).toBeNull();
    const [spec] = mocked.ensureCodexSession.mock.calls[0]!;
    expect(spec.approval).toBe('auto-review');
    expect(spec.sandbox).not.toBe('danger-full-access');
  });
});

describe('agent bridge: rows the core changed', () => {
  it('drops the row of a helper the core deleted', async () => {
    useChats.getState().addChat('/repo', { ...parent, id: 'child', parentId: 'parent' }, false);
    await mountBridge();
    emit('agent://deleted', { id: 'child' });
    expect(useChats.getState().findChat('child')).toBeNull();
  });

  it('moves a row under its new parent', async () => {
    useChats.getState().addChat('/repo', { ...parent, id: 'child', parentId: 'gone' }, false);
    await mountBridge();
    emit('agent://reparent', { id: 'child', parent: 'parent' });
    expect(useChats.getState().findChat('child')?.parentId).toBe('parent');
  });

  it('stops listening once unmounted', async () => {
    const view = await mountBridge();
    await vi.waitFor(() => expect(events.handlers.size).toBe(4));
    view.unmount();
    expect(events.handlers.size).toBe(0);
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
    parent: 'parent',
    name: 'background job',
    settings: { model: 'haiku' },
    tools: false,
    ...over
  });

  it('makes a row under its parent, keeping its id and session, and lets the core release it', async () => {
    mocked.backgroundPromote.mockResolvedValue(helper());
    await openAsChat('bg1');
    expect(useChats.getState().findChat('bg1')).toMatchObject({
      parentId: 'parent',
      model: 'Haiku',
      sessionId: 'sess-1',
      name: 'background job'
    });
    expect(mocked.backgroundPromoted).toHaveBeenCalledWith('bg1', null);
  });

  it('finds the account whatever the path’s slashes and case', async () => {
    mocked.backgroundPromote.mockResolvedValue(helper({ accountPath: '\\ACCOUNTS\\claude\\work\\' }));
    await openAsChat('bg1');
    expect(useChats.getState().findChat('bg1')).not.toBeNull();
  });

  it('tells the core why, and leaves the helper as it was, when no row can be made', async () => {
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
});
