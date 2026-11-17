import { render } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Account } from '../../shared/types';

/** A login start that answers when the test says, and the listeners' readiness likewise. */
const login = vi.hoisted(() => ({
  start: null as null | { answer: (backlog: string) => void; fail: (e: Error) => void },
  ready: Promise.resolve()
}));

vi.mock('@xterm/xterm', () => ({
  Terminal: class {
    cols = 80;
    rows = 24;
    loadAddon() {}
    open() {}
    write() {}
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
vi.mock('../panes/terminal/ptyRoutes', () => ({ routePty: () => ({ ready: login.ready, unroute() {} }) }));
vi.mock('../../ipc/sessions', () => ({
  killSession: vi.fn(() => Promise.resolve()),
  resizeSession: vi.fn(() => Promise.resolve()),
  writeSession: vi.fn(() => Promise.resolve())
}));
vi.mock('../providers', () => ({
  ui: () => ({
    loginSession: vi.fn(
      () =>
        new Promise<string>((answer, fail) => {
          login.start = { answer, fail };
        })
    )
  })
}));
vi.mock('./accounts.store', () => ({
  useAccounts: { getState: () => ({ setLoginFor() {}, refresh: () => Promise.resolve() }) }
}));

const { killSession } = vi.mocked(await import('../../ipc/sessions'));
const { LoginModal } = await import('./LoginModal');

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
