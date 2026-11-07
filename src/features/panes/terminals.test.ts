import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Chat } from '../../shared/types';

/**
 * Just enough of xterm for the pool and its pieces: writes queue until
 * `flush`, as xterm parses them on its own schedule, and what the terminal
 * would emit (typing, a resize, a screen switch) is fired by hand.
 */
const xterm = vi.hoisted(() => {
  type Listener<T> = (value: T) => void;
  const emitter = <T>() => {
    const listeners = new Set<Listener<T>>();
    return {
      on: (cb: Listener<T>) => {
        listeners.add(cb);
        return { dispose: () => listeners.delete(cb) };
      },
      fire: (value: T) => listeners.forEach(cb => cb(value))
    };
  };
  class FakeTerminal {
    static made: FakeTerminal[] = [];
    cols = 80;
    rows = 24;
    options: Record<string, unknown>;
    /** Every call that changes what the terminal shows, in order. */
    log: string[] = [];
    pending: Array<{ data: string; done?: () => void }> = [];
    disposed = false;
    data = emitter<string>();
    resized = emitter<{ cols: number; rows: number }>();
    bufferChange = emitter<void>();
    buffer = { onBufferChange: this.bufferChange.on };
    onData = this.data.on;
    onResize = this.resized.on;
    constructor(options: Record<string, unknown>) {
      this.options = options;
      FakeTerminal.made.push(this);
    }
    open() {}
    loadAddon() {}
    attachCustomKeyEventHandler() {}
    scrollToBottom() {}
    refresh() {}
    hasSelection() {
      return false;
    }
    write(data: string, done?: () => void) {
      this.log.push(`write:${data}`);
      this.pending.push({ data, done });
    }
    /** xterm gets round to parsing what was written. */
    flush() {
      const queued = this.pending;
      this.pending = [];
      queued.forEach(w => w.done?.());
    }
    reset() {
      this.log.push('reset');
    }
    dispose() {
      this.disposed = true;
    }
  }
  return { FakeTerminal };
});

vi.mock('@xterm/xterm', () => ({ Terminal: xterm.FakeTerminal }));
vi.mock('@xterm/addon-fit', () => ({
  FitAddon: class FitAddon {
    fit() {}
  }
}));
vi.mock('@xterm/addon-webgl', () => ({
  WebglAddon: class WebglAddon {
    onContextLoss() {
      return { dispose() {} };
    }
    dispose() {}
  }
}));

/** The pty's events as the terminals subscribed to them. While `hold` is on,
 *  a subscription's answer waits in `waiting` until `release`. */
const pty = vi.hoisted(() => {
  const output = new Set<(p: { id: string; data: string }) => void>();
  const exit = new Set<(p: { id: string; code: number | null }) => void>();
  const state = { hold: false, waiting: [] as Array<() => void>, unlistened: 0 };
  const release = () => {
    state.hold = false;
    state.waiting.splice(0).forEach(go => go());
  };
  const subscribe =
    <T>(set: Set<(p: T) => void>) =>
    (cb: (p: T) => void) => {
      set.add(cb);
      const unlisten = () => {
        state.unlistened++;
        set.delete(cb);
      };
      if (!state.hold) return Promise.resolve(unlisten);
      return new Promise<() => void>(resolve => state.waiting.push(() => resolve(unlisten)));
    };
  return { output, exit, state, release, onPtyOutput: subscribe(output), onPtyExit: subscribe(exit) };
});

vi.mock('../../ipc/events', () => ({ onPtyOutput: pty.onPtyOutput, onPtyExit: pty.onPtyExit }));
vi.mock('../../ipc/sessions', () => ({
  ensureClaudeSession: vi.fn(),
  ensureCodexSession: vi.fn(),
  writeSession: vi.fn(() => Promise.resolve()),
  resizeSession: vi.fn(() => Promise.resolve()),
  sessionModes: vi.fn(() => Promise.resolve(''))
}));

const sessions = vi.mocked(await import('../../ipc/sessions'));
const { acquire, forget, forgetAll } = await import('./terminals');

// jsdom has no ResizeObserver; the pool only needs it to exist.
globalThis.ResizeObserver ??= class {
  observe() {}
  disconnect() {}
  unobserve() {}
} as unknown as typeof ResizeObserver;

const chat = (id: string): Chat => ({
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
  perm: 'Bypass'
});

/** A session start that answers when the test says. */
function deferredStart() {
  let answer: (backlog: string) => void = () => {};
  sessions.ensureClaudeSession.mockImplementationOnce(() => new Promise<string>(resolve => (answer = resolve)));
  return (backlog: string) => answer(backlog);
}

/** The terminal for a chat, as a pane would take it, and its fake xterm. */
function open(id: string) {
  const handle = acquire({ chat: chat(id), folderPath: '/repo', accountPath: '/acc/work' });
  return { handle, term: xterm.FakeTerminal.made.at(-1)! };
}

const settle = () => new Promise<void>(resolve => setTimeout(resolve, 0));
const writesFor = (id: string) => sessions.writeSession.mock.calls.filter(c => c[0] === id).map(c => c[1]);
const resizesFor = (id: string) => sessions.resizeSession.mock.calls.filter(c => c[0] === id).map(c => [c[1], c[2]]);

beforeEach(() => {
  vi.clearAllMocks();
  sessions.ensureClaudeSession.mockResolvedValue('');
  sessions.sessionModes.mockResolvedValue('');
  xterm.FakeTerminal.made = [];
  pty.state.hold = false;
  pty.state.waiting = [];
  pty.state.unlistened = 0;
});

afterEach(() => {
  forgetAll();
  pty.output.clear();
  pty.exit.clear();
});

describe('terminals: the pty behind a terminal', () => {
  it('takes no keystroke or resize until its session has started, then takes both', async () => {
    const start = deferredStart();
    const { term } = open('gate');
    await settle();
    term.data.fire('early');
    term.resized.fire({ cols: 100, rows: 30 });
    expect(writesFor('gate')).toEqual([]);
    expect(resizesFor('gate')).toEqual([]);

    start('');
    await settle();
    // A fresh session's first real size, which also makes the CLI paint.
    expect(resizesFor('gate')).toEqual([[80, 24]]);
    term.data.fire('ls\r');
    term.resized.fire({ cols: 100, rows: 30 });
    expect(writesFor('gate')).toEqual(['ls\r']);
    expect(resizesFor('gate')).toEqual([
      [80, 24],
      [100, 30]
    ]);
  });

  it('stops sending once the session has exited, and says so on screen', async () => {
    const { term } = open('exits');
    await settle();
    pty.exit.forEach(cb => cb({ id: 'exits', code: 0 }));
    term.data.fire('x');
    expect(writesFor('exits')).toEqual([]);
    expect(term.log.at(-1)).toContain('[session exited]');
  });

  it('sends nothing for a session that failed to start, and shows why', async () => {
    sessions.ensureClaudeSession.mockRejectedValueOnce(new Error('no such account'));
    const { term } = open('fails');
    await settle();
    term.data.fire('x');
    expect(writesFor('fails')).toEqual([]);
    expect(term.log.at(-1)).toContain('failed to start session: no such account');
  });
});

describe('terminals: reattaching to a running session', () => {
  it('resets only once the replayed tail is parsed, then puts the modes back, then asks for a repaint', async () => {
    sessions.ensureClaudeSession.mockResolvedValueOnce('TAIL');
    sessions.sessionModes.mockResolvedValueOnce('MODES');
    const { term } = open('replay');
    await settle();
    // Queued, not parsed: a reset now would clear an empty screen and the
    // stale tail would land on top of it.
    expect(term.log).toEqual(['write:TAIL']);
    expect(sessions.sessionModes).not.toHaveBeenCalled();

    term.flush();
    await settle();
    expect(term.log).toEqual(['write:TAIL', 'reset', 'write:MODES']);
    // The repaint: one row short, then the real size once the layout settles.
    expect(resizesFor('replay')[0]).toEqual([80, 23]);
    await vi.waitFor(() =>
      expect(resizesFor('replay')).toEqual([
        [80, 23],
        [80, 24]
      ])
    );
  });

  it('does nothing more to a terminal disposed while the tail was being parsed', async () => {
    sessions.ensureClaudeSession.mockResolvedValueOnce('TAIL');
    const { term } = open('replay-gone');
    await settle();
    forget('replay-gone');
    term.flush();
    await settle();
    expect(term.log).toEqual(['write:TAIL']);
    expect(sessions.sessionModes).not.toHaveBeenCalled();
  });
});

describe('terminals: disposed while starting', () => {
  it('starts no session when disposed before its event subscriptions are in, and drops them as they arrive', async () => {
    pty.state.hold = true;
    const { term } = open('early-gone');
    forget('early-gone');
    expect(term.disposed).toBe(true);
    pty.release();
    await settle();
    expect(sessions.ensureClaudeSession).not.toHaveBeenCalled();
    expect(pty.state.unlistened).toBe(2);
  });

  it('writes nothing and sends nothing when disposed while its session was starting', async () => {
    const start = deferredStart();
    const { term } = open('mid-gone');
    await settle();
    forget('mid-gone');
    expect(pty.state.unlistened).toBe(2);
    start('TAIL');
    await settle();
    term.data.fire('x');
    expect(term.log).toEqual([]);
    expect(writesFor('mid-gone')).toEqual([]);
    expect(resizesFor('mid-gone')).toEqual([]);
  });
});

describe('terminals: two chats at once', () => {
  it('keeps each one’s output, keystrokes and exit to itself', async () => {
    const a = open('chat-a');
    const b = open('chat-b');
    await settle();
    pty.output.forEach(cb => cb({ id: 'chat-a', data: 'for a' }));
    expect(a.term.log).toEqual(['write:for a']);
    expect(b.term.log).toEqual([]);

    b.term.data.fire('typed in b');
    expect(writesFor('chat-b')).toEqual(['typed in b']);
    expect(writesFor('chat-a')).toEqual([]);

    pty.exit.forEach(cb => cb({ id: 'chat-a', code: 0 }));
    b.term.data.fire('still live');
    a.term.data.fire('gone');
    expect(writesFor('chat-b')).toEqual(['typed in b', 'still live']);
    expect(writesFor('chat-a')).toEqual([]);
  });

  it('gives back the same terminal to the chat that asks again, and a separate one to another', () => {
    const first = open('same').handle;
    expect(acquire({ chat: chat('same'), folderPath: '/repo', accountPath: '/acc/work' })).toBe(first);
    expect(open('other').handle).not.toBe(first);
  });
});
