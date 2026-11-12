import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import * as handoffIpc from '../../ipc/handoff';
import type { MoveStatusDto } from '../../ipc/moving';
import type { Account, Chat } from '../../shared/types';
import { useAccounts } from '../accounts/accounts.store';
import { RESUME_LOOK_MS } from '../chats/autoHandoff';
import { useChats } from '../chats/chats.store';
import { ASK_HOLDS_MS, handoffNowRefusal, useHandoffNow } from '../chats/handoffNow';
import { handoffNowItem, PaneMenu } from './Pane';
import { useRestarts } from './restarts.store';
import * as terminals from './terminals';

vi.mock('../../ipc/handoff', async importOriginal => ({
  ...(await importOriginal<typeof import('../../ipc/handoff')>()),
  handoffNow: vi.fn(() => Promise.resolve({ resume: false }))
}));
vi.mock('./terminals', async importOriginal => ({
  ...(await importOriginal<typeof import('./terminals')>()),
  resumeSession: vi.fn(() => Promise.resolve())
}));

function mount() {
  const onClose = vi.fn();
  const run = vi.fn();
  const { rerender } = render(
    <>
      <button>More</button>
      <button>elsewhere</button>
    </>
  );
  const anchor = screen.getByRole('button', { name: 'More' });
  rerender(
    <>
      <button>More</button>
      <button>elsewhere</button>
      <PaneMenu
        anchor={anchor}
        onClose={onClose}
        items={[
          { label: 'Rename', title: 'Rename chat', run },
          { label: 'Delete chat…', title: 'Asks first', run }
        ]}
      />
    </>
  );
  return { anchor, onClose, run };
}

describe('PaneMenu', () => {
  it('closes when focus leaves it', async () => {
    const user = userEvent.setup();
    const { onClose } = mount();
    expect(screen.getByRole('menuitem', { name: 'Rename' })).toHaveFocus();
    await user.click(screen.getByRole('button', { name: 'elsewhere' }));
    expect(onClose).toHaveBeenCalled();
  });

  it('takes Escape only inside it, and gives focus back to its button then', () => {
    const { anchor, onClose } = mount();
    const elsewhere = screen.getByRole('button', { name: 'elsewhere' });
    // Focus elsewhere with the menu still mounted: onClose is a mock here.
    act(() => elsewhere.focus());
    onClose.mockClear();
    const reached = vi.fn((e: KeyboardEvent) => e.defaultPrevented);
    elsewhere.addEventListener('keydown', reached);
    fireEvent.keyDown(elsewhere, { key: 'Escape' });
    expect(reached.mock.results[0]!.value).toBe(false);
    expect(onClose).not.toHaveBeenCalled();
    expect(elsewhere).toHaveFocus();

    act(() => screen.getByRole('menuitem', { name: 'Rename' }).focus());
    fireEvent.keyDown(screen.getByRole('menuitem', { name: 'Rename' }), { key: 'Escape' });
    expect(onClose).toHaveBeenCalled();
    expect(anchor).toHaveFocus();
  });
});

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
const chat = (extra: Partial<Chat>): Chat =>
  ({
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
    perm: 'Edits',
    tools: true,
    ...extra
  }) as Chat;

/** The menu holding only Hand off now for `c`, as the pane offers it. */
function menuFor(c: Chat, why = handoffNowRefusal(c, [])) {
  const onClose = vi.fn();
  render(<button>More</button>);
  const anchor = screen.getByRole('button', { name: 'More' });
  render(<PaneMenu anchor={anchor} onClose={onClose} items={[handoffNowItem(c, why)]} />);
  return { item: screen.getByRole('menuitem', { name: 'Hand off now' }), onClose };
}

describe('Hand off now', () => {
  afterEach(() => {
    vi.useRealTimers();
    useHandoffNow.setState({ asked: {}, refused: {} });
    useRestarts.setState({ by: {}, pending: {} });
    vi.mocked(handoffIpc.handoffNow).mockClear();
    vi.mocked(terminals.resumeSession).mockClear();
  });

  it('asks a chat with Luna tools for a handoff, auto-handoff or not, and says what it does', async () => {
    const { item, onClose } = menuFor(chat({ autoHandoff: false }));
    expect(item).not.toHaveAttribute('aria-disabled');
    expect(item).toHaveAttribute('title', expect.stringMatching(/fresh conversation.*Nothing is reset unless/));
    await userEvent.setup().click(item);
    expect(onClose).toHaveBeenCalled();
    expect(handoffIpc.handoffNow).toHaveBeenCalledWith('c1');
    await waitFor(() => expect(useHandoffNow.getState().asked.c1).toBeTypeOf('number'));
    expect(terminals.resumeSession).not.toHaveBeenCalled();
  });

  it('is not offered, with why as its title, where the chat could not answer', async () => {
    const move = (phase: MoveStatusDto['phase']) => ({ moveId: 'm', chatId: 'c1', newId: 'n', phase }) as MoveStatusDto;
    expect(handoffNowRefusal(chat({ tools: false }), [])).toMatch(/Luna tools box ticked/);
    expect(handoffNowRefusal(chat({ perm: 'Plan only' }), [])).toMatch(/leave plan mode/);
    expect(handoffNowRefusal(chat({ provider: 'codex', sandbox: 'read-only', approval: 'on-request' }), [])).toMatch(
      /read-only sandbox/
    );
    expect(handoffNowRefusal(chat({ handoff: { n: 3, at: 1, state: 'underWay', previous: false } }), [])).toMatch(
      /Handoff 3 .* under way/
    );
    expect(handoffNowRefusal(chat({}), [move('asked')])).toMatch(/move's document/);
    expect(handoffNowRefusal(chat({}), [move('done')])).toBeNull();
    // Asked already: until it lapses, or a handoff came of it.
    expect(handoffNowRefusal(chat({}), [], 1000, 2000)).toMatch(/Asked already/);
    expect(handoffNowRefusal(chat({}), [], 1000, 1000 + ASK_HOLDS_MS + 1)).toBeNull();
    expect(
      handoffNowRefusal(chat({ handoff: { n: 1, at: 1500, state: 'done', previous: true } }), [], 1000, 2000)
    ).toBeNull();

    const { item, onClose } = menuFor(chat({ tools: false }));
    expect(item).toHaveAttribute('aria-disabled', 'true');
    expect(item).toHaveAttribute('title', expect.stringMatching(/without Luna's tools/));
    await userEvent.setup().click(item);
    expect(onClose).not.toHaveBeenCalled();
    expect(handoffIpc.handoffNow).not.toHaveBeenCalled();
  });

  it("keeps the core's refusal for the pane to show", async () => {
    vi.mocked(handoffIpc.handoffNow).mockRejectedValueOnce("the chat's CLI has its own jobs running (Bash)");
    const { item } = menuFor(chat({}));
    await userEvent.setup().click(item);
    await waitFor(() =>
      expect(useHandoffNow.getState().refused.c1).toBe("the chat's CLI has its own jobs running (Bash)")
    );
    expect(useHandoffNow.getState().asked.c1).toBeUndefined();
  });

  it('starts a CLI that lacks the tool again once the chat is idle, auto-handoff off', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    vi.mocked(handoffIpc.handoffNow).mockResolvedValueOnce({ resume: true });
    useAccounts.setState({ accounts: [account], loaded: true });
    useChats.setState({
      folders: [{ id: 'f', path: '/repo', open: true, chats: [chat({ status: 'working', autoHandoff: false })] }]
    });
    const { item } = menuFor(chat({ status: 'working' }));
    act(() => item.click());
    await waitFor(() => expect(useRestarts.getState().pending.c1).toBe(true));
    await act(() => vi.advanceTimersByTimeAsync(RESUME_LOOK_MS * 2));
    expect(terminals.resumeSession).not.toHaveBeenCalled();
    act(() => useChats.getState().setStatus('c1', 'resting'));
    await act(() => vi.advanceTimersByTimeAsync(RESUME_LOOK_MS * 2));
    expect(terminals.resumeSession).toHaveBeenCalledTimes(1);
  });
});
