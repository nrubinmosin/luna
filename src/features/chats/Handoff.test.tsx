import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import * as handoffIpc from '../../ipc/handoff';
import * as terminals from '../panes/terminals';
import { handoffOf, type Account, type Chat, type HandoffMark } from '../../shared/types';
import { useAccounts } from '../accounts/accounts.store';
import { useRestarts } from '../panes/restarts.store';
import { RESUME_LOOK_MS } from './autoHandoff';
import { useChats } from './chats.store';
import { clampThreshold, HandoffBar, HandoffBox, markWords } from './Handoff';

vi.mock('../../ipc/handoff', () => ({
  setAutoHandoff: vi.fn(() => Promise.resolve({ resume: false })),
  handoffFile: vi.fn(() => Promise.resolve('# Objective\nShip it.')),
  handoffTranscript: vi.fn(() => Promise.resolve({ messages: [], cursor: 0, file: null, running: false }))
}));
// xterm cannot run in jsdom.
vi.mock('../panes/terminals', () => ({
  startSession: vi.fn(),
  resumeSession: vi.fn(() => Promise.resolve()),
  focusTerminal: vi.fn(),
  forget: vi.fn(),
  forgetAll: vi.fn()
}));

const account: Account = {
  provider: 'claude',
  name: 'work',
  path: '/accounts/claude/work',
  plan: 'max',
  email: null,
  signedIn: true,
  haveUsage: false,
  limits: [],
  usageAge: null,
  fetchedAt: null,
  sync: 'ready',
  refreshError: null
};

const base = {
  id: 'c1',
  name: 'campaign',
  provider: 'claude',
  account: 'work',
  status: 'resting',
  context: 0.4,
  group: 0,
  worktree: false,
  model: 'Opus',
  effort: 'high',
  perm: 'Bypass'
} as Chat;

function seat(chat: Partial<Chat>) {
  useChats.setState({ folders: [{ id: 'f', path: '/repo', open: true, chats: [{ ...base, ...chat } as Chat] }] });
}

beforeEach(() => {
  useAccounts.setState({ accounts: [account], loaded: true });
  useRestarts.setState({ by: {}, pending: {} });
  vi.mocked(handoffIpc.setAutoHandoff).mockClear();
  vi.mocked(terminals.resumeSession).mockClear();
});

afterEach(() => {
  vi.useRealTimers();
});

describe('handoff threshold', () => {
  it('goes to the CLI only with Luna tools and the box ticked', () => {
    expect(handoffOf({ tools: true, autoHandoff: true, handoffAt: 55 })).toBe(55);
    expect(handoffOf({ tools: true, autoHandoff: true })).toBe(70);
    expect(handoffOf({ tools: false, autoHandoff: true, handoffAt: 55 })).toBeNull();
    expect(handoffOf({ tools: true, autoHandoff: false, handoffAt: 55 })).toBeNull();
  });

  it('is held to the range the window offers', () => {
    expect([clampThreshold(5), clampThreshold(95), clampThreshold(72.6), clampThreshold(NaN)]).toEqual([
      10, 90, 73, 70
    ]);
  });
});

describe('HandoffBar', () => {
  const mark = (m: Partial<HandoffMark>): HandoffMark => ({
    n: 2,
    at: Date.UTC(2026, 10, 9, 10, 5),
    state: 'done',
    previous: true,
    ...m
  });

  it('says where the latest handoff stands', () => {
    expect(markWords(mark({}))).toMatch(/^Handoff 2 · \d\d:\d\d/);
    expect(markWords(mark({ state: 'underWay' }))).toMatch(/^Handoff 2 · under way since /);
    expect(markWords(mark({ state: 'failed' }))).toMatch(/^Handoff 2 did not go through/);
  });

  it('is not there before a first handoff, and opens the copy and the conversation before', async () => {
    const { container, rerender } = render(<HandoffBar chat={base} />);
    expect(container).toBeEmptyDOMElement();
    const chat = { ...base, handoff: mark({}) };
    rerender(<HandoffBar chat={chat} />);
    expect(screen.getByText('previous conversation')).toBeInTheDocument();
    const user = userEvent.setup();
    await user.click(screen.getByText('open handoff'));
    await waitFor(() => expect(screen.getByText(/Ship it\./)).toBeInTheDocument());
    expect(handoffIpc.handoffFile).toHaveBeenCalledWith('c1', 2);
  });

  it('offers no conversation before a handoff that did not go through', () => {
    render(<HandoffBar chat={{ ...base, handoff: mark({ state: 'failed', why: 'no new conversation' }) }} />);
    expect(screen.queryByText('previous conversation')).toBeNull();
    expect(screen.getByRole('note')).toHaveAttribute('title', 'no new conversation');
  });
});

describe('HandoffBox', () => {
  it('needs Luna tools', () => {
    seat({ tools: false });
    render(<HandoffBox chat={{ ...base, tools: false }} />);
    expect(screen.getByLabelText('Auto-handoff at')).toBeDisabled();
  });

  it('tells the core at once, and keeps it on the chat', async () => {
    seat({ tools: true });
    const user = userEvent.setup();
    render(<HandoffBox chat={{ ...base, tools: true }} />);
    await user.click(screen.getByLabelText('Auto-handoff at'));
    await waitFor(() => expect(handoffIpc.setAutoHandoff).toHaveBeenCalledWith('c1', 70));
    expect(useChats.getState().findChat('c1')).toMatchObject({ autoHandoff: true, handoffAt: 70 });
    expect(terminals.resumeSession).not.toHaveBeenCalled();
    await user.click(screen.getByLabelText('Auto-handoff at'));
    await waitFor(() => expect(handoffIpc.setAutoHandoff).toHaveBeenLastCalledWith('c1', null));
  });

  it('resumes a running chat whose CLI lacks the tool, once it is idle', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    vi.mocked(handoffIpc.setAutoHandoff).mockResolvedValueOnce({ resume: true });
    seat({ tools: true, status: 'working' });
    render(<HandoffBox chat={{ ...base, tools: true }} />);
    act(() => screen.getByLabelText('Auto-handoff at').click());
    await waitFor(() => expect(useRestarts.getState().pending.c1).toBe(true));
    expect(screen.getByText(/waiting for it to be idle/)).toBeInTheDocument();
    await act(() => vi.advanceTimersByTimeAsync(RESUME_LOOK_MS * 3));
    expect(terminals.resumeSession).not.toHaveBeenCalled();
    act(() => useChats.getState().setStatus('c1', 'resting'));
    await act(() => vi.advanceTimersByTimeAsync(RESUME_LOOK_MS * 2));
    expect(terminals.resumeSession).toHaveBeenCalledTimes(1);
    expect(vi.mocked(terminals.resumeSession).mock.calls[0]?.[1]).toBe('/repo');
    await waitFor(() => expect(useRestarts.getState().pending.c1).toBeUndefined());
  });
});
