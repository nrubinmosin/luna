import { Terminal } from '@xterm/xterm';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { logWarn } from '../../../shared/lib/log';
import { loadWebgl } from './webgl';

/**
 * The webgl addon stood in for (jsdom has no WebGL), loaded into a real xterm,
 * whose own addon bookkeeping is what a dispose goes through.
 */
const webgl = vi.hoisted(() => {
  class FakeWebglAddon {
    static made: FakeWebglAddon[] = [];
    static failToLoad: Error | null = null;
    losses = new Set<() => void>();
    /** Each run of the addon's own teardown. */
    teardowns = 0;
    failToDispose: Error | null = null;
    constructor() {
      FakeWebglAddon.made.push(this);
    }
    onContextLoss(cb: () => void) {
      this.losses.add(cb);
      return { dispose: () => this.losses.delete(cb) };
    }
    activate() {
      if (FakeWebglAddon.failToLoad) throw FakeWebglAddon.failToLoad;
    }
    dispose() {
      this.teardowns++;
      this.losses.clear();
      if (this.failToDispose) throw this.failToDispose;
    }
    /** The browser takes the context away for good. */
    loseContext() {
      [...this.losses].forEach(cb => cb());
    }
  }
  return { FakeWebglAddon };
});

vi.mock('@xterm/addon-webgl', () => ({ WebglAddon: webgl.FakeWebglAddon }));
vi.mock('../../../shared/lib/log', () => ({ logWarn: vi.fn() }));

/** The addon loadWebgl made. */
const addon = () => {
  const made = webgl.FakeWebglAddon.made[0];
  if (!made) throw new Error('no webgl addon was made');
  return made;
};
let term: Terminal;

beforeEach(() => {
  webgl.FakeWebglAddon.made = [];
  webgl.FakeWebglAddon.failToLoad = null;
  vi.mocked(logWarn).mockClear();
  term = new Terminal();
});

afterEach(() => {
  term.dispose();
  vi.unstubAllGlobals();
});

describe('loadWebgl', () => {
  it('disposes the addon once, however often it is asked, and the terminal skips it after', () => {
    const dispose = loadWebgl(term);
    dispose();
    dispose();
    term.dispose();
    expect(addon().teardowns).toBe(1);
  });

  it('on a lost context, takes the addon off first and then has the pane repainted', () => {
    const order: string[] = [];
    const addonGone = () => (addon().teardowns ? 'after' : 'before');
    const dispose = loadWebgl(term, () => order.push(`repaint ${addonGone()} the dispose`));
    addon().loseContext();
    expect(order).toEqual(['repaint after the dispose']);
    dispose();
    term.dispose();
    expect(addon().teardowns).toBe(1);
  });

  it('logs a dispose that throws, of whatever kind, and keeps it from the caller', () => {
    const dispose = loadWebgl(term);
    addon().failToDispose = new TypeError('broken teardown');
    expect(dispose).not.toThrow();
    expect(vi.mocked(logWarn)).toHaveBeenCalledWith('terminal', expect.stringContaining('broken teardown'));
  });

  it('leaves the terminal on the DOM renderer where the addon cannot load, and drops it', () => {
    webgl.FakeWebglAddon.failToLoad = new Error('WebGL2 not supported');
    const dispose = loadWebgl(term);
    expect(vi.mocked(logWarn)).toHaveBeenCalledWith('terminal', expect.stringContaining('WebGL2 not supported'));
    expect(addon().teardowns).toBe(1);
    dispose();
    expect(addon().teardowns).toBe(1);
  });
});

/**
 * What @xterm/addon-webgl's teardown does to put the terminal back on its DOM
 * renderer, against the xterm installed: the addon reaches into xterm's
 * internals for it, and an xterm the addon was not made for (5.5 under the
 * addon's 0.19) threw a TypeError there and left the pane blank.
 */
interface Core {
  _store: { _isDisposed: boolean };
  _createRenderer(): unknown;
  _renderService: { setRenderer(renderer: unknown): void; handleResize(cols: number, rows: number): void };
}

describe('the webgl teardown against this xterm', () => {
  it('finds what it reaches for and puts a DOM renderer in', () => {
    // What xterm asks of the window as it opens and jsdom lacks.
    vi.stubGlobal('matchMedia', (media: string) => ({
      matches: false,
      media,
      addEventListener() {},
      removeEventListener() {},
      addListener() {},
      removeListener() {}
    }));
    const host = document.createElement('div');
    document.body.appendChild(host);
    term.open(host);
    const core = (term as unknown as { _core: Core })._core;
    expect(core._store._isDisposed).toBe(false);
    const renderService = core._renderService;
    renderService.setRenderer(core._createRenderer());
    renderService.handleResize(term.cols, term.rows);
    expect(host.querySelector('.xterm-rows')).not.toBeNull();
    host.remove();
  });
});
