import { render } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Account } from '../../shared/types';

/** A login start that answers when the test says, and the listeners' readiness likewise. */
const login = vi.hoisted(() => ({
  start: null as null | { answer: (backlog: string) => void; fail: (e: Error) => void },
  ready: Promise.resolve(),
  /** The routes, starts and kills, in the order they were asked for. */
  order: [] as string[],
  /** What the windows' terminals were given to show. */
  shown: [] as string[],
  /** The core's session of the id, as pty/spawn.rs keeps it: a start makes
   *  one before it answers, a start while one lives gets that one back, and a
   *  kill takes it out. */
  live: null as null | number,
  made: 0
}));

vi.mock('@xterm/xterm', () => ({
  Terminal: class {
    cols = 80;
    rows = 24;
    loadAddon() {}
    open() {}
    write(data: string) {
      login.shown.push(data);
    }
    onData() {
      return { dispose() {} };
    }
    onResize() {
      return { dispose() {} };
    }
    dispose() {}
  }
}));
vi.mock('@xterm/addon-fit', () => ({ FitAddon: class {} }));
vi.mock('../panes/terminal/webgl', () => ({ loadWebgl: () => () => {} }));
vi.mock('../panes/terminal/keys', () => ({ attachLoginKeys() {} }));
vi.mock('../panes/terminal/ptyRoutes', () => ({
  routePty: () => {
    login.order.push('route');
    return { ready: login.ready, unroute() {} };
  }
}));
vi.mock('../../ipc/sessions', () => ({
  killSession: vi.fn(() => {
    login.order.push('kill');
    login.live = null;
    return Promise.resolve();
  }),
  resizeSession: vi.fn(() => Promise.resolve()),
  writeSession: vi.fn(() => Promise.resolve())
}));
vi.mock('../providers', () => ({
  ui: () => ({
    loginSession: vi.fn(() => {
      if (login.live !== null) {
        login.order.push('reuse');
        return Promise.resolve('');
      }
      login.live = ++login.made;
      return new Promise<string>((answer, fail) => {
        login.order.push('start');
        login.start = { answer, fail };
      });
    })
  })
}));
vi.mock('./accounts.store', () => ({
  useAccounts: { getState: () => ({ setLoginFor() {}, refresh: () => Promise.resolve() }) }
}));

const { killSession } = vi.mocked(await import('../../ipc/sessions'));
const { ENDING_WAIT_MS, LoginModal } = await import('./LoginModal');

globalThis.ResizeObserver ??= class {
  observe() {}
  disconnect() {}
  unobserve() {}
} as unknown as typeof ResizeObserver;

const account = { provider: 'claude', name: 'work', path: '/acc/work' } as Account;
const settle = () => new Promise<void>(resolve => setTimeout(resolve, 0));

beforeEach(() => {
  vi.clearAllMocks();
  login.start = null;
  login.ready = Promise.resolve();
  login.order = [];
  login.shown = [];
  login.live = null;
  login.made = 0;
});

describe('LoginModal: closed while its session is starting', () => {
  it('ends the session once the start has answered, not before', async () => {
    const { unmount } = render(<LoginModal account={account} />);
    await settle();
    expect(login.start).not.toBeNull();
    unmount();
    await settle();
    expect(killSession).not.toHaveBeenCalled();

    login.start!.answer('');
    await settle();
    expect(killSession).toHaveBeenCalledExactlyOnceWith('login:claude:work');
  });

  it('has a window opened again on the account listen and start its own once that kill has landed', async () => {
    const first = render(<LoginModal account={account} />);
    await settle();
    const firstStart = login.start!;
    first.unmount();
    // The first window's process is still starting, its kill not yet sent:
    // the second must neither hear it nor attach to it.
    const second = render(<LoginModal account={account} />);
    await settle();
    expect(login.order).toEqual(['route', 'start']);

    firstStart.answer('');
    await settle();
    expect(login.order).toEqual(['route', 'start', 'kill', 'route', 'start']);
    expect(killSession).toHaveBeenCalledExactlyOnceWith('login:claude:work');
    expect(login.live).toBe(2);
    // Its own start answered and ended, as it would be, so the next window
    // waits on nothing.
    second.unmount();
    login.start!.answer('');
    await settle();
  });

  it('waits past the deadline for a start slow to answer, and never gets that session back', async () => {
    vi.useFakeTimers();
    try {
      const first = render(<LoginModal account={account} />);
      await vi.advanceTimersByTimeAsync(0);
      const firstStart = login.start!;
      // Closed while its start is slow to answer: the core already holds
      // its session.
      first.unmount();
      const second = render(<LoginModal account={account} />);
      await vi.advanceTimersByTimeAsync(ENDING_WAIT_MS - 1);
      expect(login.shown.join('')).not.toContain('has not ended yet');
      await vi.advanceTimersByTimeAsync(1);
      expect(login.order).toEqual(['route', 'start']);
      expect(login.shown.join('')).toContain('has not ended yet');
      await vi.advanceTimersByTimeAsync(60_000);
      expect(login.order).toEqual(['route', 'start']);

      // The start answers at last, the kill takes that session out, and only
      // then does the second window start its own, which stays.
      firstStart.answer('');
      await vi.advanceTimersByTimeAsync(0);
      expect(login.order).toEqual(['route', 'start', 'kill', 'route', 'start']);
      expect(login.live).toBe(2);
      second.unmount();
      login.start!.answer('');
      await vi.advanceTimersByTimeAsync(0);
    } finally {
      vi.useRealTimers();
    }
  });

  it('kills nothing when the start failed', async () => {
    const { unmount } = render(<LoginModal account={account} />);
    await settle();
    unmount();
    login.start!.fail(new Error('no CLI'));
    await settle();
    expect(killSession).not.toHaveBeenCalled();
  });

  it('starts no session when closed before its output could be heard', async () => {
    let ready: () => void = () => {};
    login.ready = new Promise<void>(resolve => (ready = resolve));
    const { unmount } = render(<LoginModal account={account} />);
    unmount();
    ready();
    await settle();
    expect(login.start).toBeNull();
    expect(killSession).not.toHaveBeenCalled();
  });
});
