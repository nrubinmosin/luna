/**
 * The live terminals, kept a little past the moment a pane stops showing one.
 *
 * A pane used to own its xterm outright, so switching what a pane showed threw
 * the terminal away and built another: an `ensure_session` round trip carrying
 * up to 2MB of scrollback, a reset, and then a wait for the CLI to repaint.
 * With a single pane on screen that is the cost of changing chat at all, which
 * is the one thing the sidebar click exists to do.
 *
 * So a terminal outlives the pane that showed it. It is made once per chat, and
 * when no pane wants it the element is parked off-screen rather than destroyed:
 * it keeps its listeners, keeps taking the session's output, and stays exactly
 * as current as the pty behind it. Coming back is a refit and a repaint nudge.
 * Only WARM of them wait like that; past the limit the one put down longest ago
 * is disposed for real, which costs nothing beyond the next reattach.
 *
 * Parked, not detached and not hidden: xterm's renderers want a real box to
 * draw into, and a `display:none` host measures zero — which is the one thing
 * the fit addon must never be shown.
 */
import { Terminal as XTerm } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import { WebglAddon } from '@xterm/addon-webgl';
import '@xterm/xterm/css/xterm.css';
import { logWarn } from '../../shared/lib/log';
import type { Chat } from '../../shared/types';
import { ensureSession, resizeSession, writeSession } from '../../ipc/commands';
import { onPtyExit, onPtyOutput } from '../../ipc/events';
import { useChats } from '../chats/chats.store';

/** How many terminals stay alive with no pane showing them. */
const WARM = 3;

export const dark = () => document.querySelector('[data-app]')?.getAttribute('data-theme') === 'dark';

export const TERM_FONT_SIZE = 14;
export const TERM_FONT_FAMILY = "'JetBrains Mono', 'Cascadia Mono', ui-monospace, monospace";

// xterm's webgl renderer and the fit addon both throw if they are poked while
// the host element is detached or already torn down. Any throw here escapes a
// React effect cleanup and kills the entire root, so every call is guarded.
export const safely = (fn: () => void) => {
  try {
    fn();
  } catch (e) {
    // With the stack: the message alone ("cannot read _isDisposed") names an
    // xterm internal and not which of the half-dozen guarded calls reached it.
    const stack = (e as Error)?.stack;
    logWarn('terminal', `teardown step failed: ${stack ?? String(e)}`);
  }
};

// A transparent background makes the webgl renderer blend every glyph against
// the pane, which is what washed the text out. Paint an opaque background that
// matches --term instead, and ship a full 16-colour ANSI ramp — without one
// xterm falls back to its stock palette, which is muddy against these panels.
export const themeFor = (isDark: boolean) =>
  isDark
    ? {
        background: '#101a1d', foreground: '#e8f4f0', cursor: '#7ef0d4',
        cursorAccent: '#101a1d', selectionBackground: '#2f5f57', selectionForeground: '#ffffff',
        black: '#2a3538', red: '#ff6b6b', green: '#78e08f', yellow: '#ffd479',
        blue: '#79b8ff', magenta: '#d2a8ff', cyan: '#5fe3d0', white: '#dfeeea',
        brightBlack: '#587076', brightRed: '#ff9492', brightGreen: '#a4f7b0', brightYellow: '#ffe9a8',
        brightBlue: '#a5d6ff', brightMagenta: '#e2c5ff', brightCyan: '#9df0e4', brightWhite: '#ffffff'
      }
    : {
        background: '#f4faf6', foreground: '#16211c', cursor: '#0b6b56',
        cursorAccent: '#f4faf6', selectionBackground: '#b6dcca', selectionForeground: '#0b1712',
        black: '#25302c', red: '#c0392b', green: '#1a7f4b', yellow: '#8a6a12',
        blue: '#1c5fa8', magenta: '#7d3ca8', cyan: '#0f736e', white: '#c8d6d0',
        brightBlack: '#5c6f68', brightRed: '#e04b3a', brightGreen: '#1fa460', brightYellow: '#a9820f',
        brightBlue: '#2a7fd4', brightMagenta: '#9b52d1', brightCyan: '#12968f', brightWhite: '#0d1512'
      };

/**
 * Keystrokes only. xterm's `onData` also carries what the terminal itself
 * answers: the cursor-position reply the CLI asks for at startup, focus in/out,
 * and an SGR mouse report for every pointer move across a pane. Dropping just
 * the escape byte left their parameters behind as text, which is how a chat
 * came to be titled `[1;1R[O[O[<35;1;14M…` — so take the whole sequence out.
 */
const typedOnly = (d: string) => d.replace(/\x1b(?:\[[\d;<>?]*[ -/]*[@-~]|O.|.)?/g, '');

export interface TermSpec {
  chat: Chat;
  folderPath: string;
  accountPath: string | null;
}

export interface TermHandle {
  /** The element to put in a pane. Moving it between panes is an appendChild. */
  host: HTMLDivElement;
  /** Fit to the box it is in now and make the CLI paint into it. */
  wake: () => void;
}

interface Entry extends TermHandle {
  chatId: string;
  /** Rebuilt if this changes: the session was spawned against this account. */
  accountPath: string | null;
  attached: boolean;
  /** Which release put it down; null while a pane is showing it. */
  idleAt: number | null;
  dispose: () => void;
}

const live = new Map<string, Entry>();

/** Ticks on every release, so "put down longest ago" stays exact when a whole
 *  board is let go at once and a clock would read the same for all of it. */
let releases = 0;

/** Where a terminal waits between panes: laid out for the renderer's sake, off
 *  screen for everyone else's. */
let lot: HTMLDivElement | null = null;
function parkingLot(): HTMLDivElement {
  if (lot?.isConnected) return lot;
  lot = document.createElement('div');
  lot.setAttribute('data-terminal-lot', '');
  lot.style.cssText =
    'position:fixed;left:-20000px;top:0;width:1200px;height:800px;overflow:hidden;pointer-events:none;';
  document.body.appendChild(lot);
  return lot;
}

function create(spec: TermSpec): Entry {
  const { chat, folderPath, accountPath } = spec;
  const chatId = chat.id;

  const host = document.createElement('div');
  host.style.cssText = 'flex:1;min-width:0;min-height:0;';
  parkingLot().appendChild(host);

  const term = new XTerm({
    fontFamily: TERM_FONT_FAMILY,
    fontSize: TERM_FONT_SIZE,
    lineHeight: 1.25,
    allowTransparency: false,
    cursorBlink: true,
    // The CLI spends most of its life fullscreen, i.e. on the alternate
    // screen, where there is no history and a scrollback buffer only bought a
    // viewport that could sit parked above the prompt. But it does leave that
    // screen — opening a shell's details prints straight onto the normal one —
    // and with no buffer at all everything scrolled past was gone for good,
    // including the CLI's own UI. Keep a buffer for those stretches; the
    // alternate screen is pinned to the bottom below.
    scrollback: 1000,
    // Nudge unreadable theme-vs-content combinations without flattening colours.
    minimumContrastRatio: 3,
    theme: themeFor(dark())
  });
  const fit = new FitAddon();
  term.loadAddon(fit);
  term.open(host);

  // Both screens have to be pinned to the bottom the moment they are switched
  // to. The alternate screen is a single painted frame that reads as a broken
  // one when parked above the prompt — and the normal screen is worse: opening
  // a shell's details drops the CLI onto it at whatever scroll position the
  // normal buffer was left at, which is the top of the conversation. The
  // details then print below the viewport, out of sight, and the CLI's own
  // footer with them. xterm follows new output on its own once the viewport is
  // at the bottom, so landing there is the whole fix.
  const pinBottom = () => term.scrollToBottom();
  pinBottom();
  const bufferSub = term.buffer.onBufferChange(pinBottom);

  let disposed = false;
  let repaintTimer: ReturnType<typeof setTimeout> | undefined;
  const unlisteners: Array<() => void> = [];
  // The pty session exists only between ensure_session resolving and
  // pty://exit. Outside that window every write/resize came straight back as
  // "no such session" — once as an ipc warning and once more as an unhandled
  // rejection — and the layout resizes a pane goes through while it is
  // opening or after its CLI exited kept both logs busy. Gate the senders on
  // the session actually being there; the .catch covers the race where the
  // pty dies while a call is in flight.
  let sessionLive = false;
  const sendResize = (cols: number, rows: number) => {
    if (!sessionLive || cols < 1 || rows < 1) return;
    resizeSession(chatId, cols, rows).catch(() => {});
  };
  const sendWrite = (data: string) => {
    if (!sessionLive) return;
    writeSession(chatId, data).catch(() => {});
  };

  const entry: Entry = {
    chatId,
    accountPath,
    host,
    attached: false,
    idleAt: 0,
    wake: () => {},
    dispose: () => {}
  };

  // Fitting a detached or zero-sized host throws; that happens on every
  // pane close and used to take the whole window down with it. Parked is the
  // other case to sit out: the lot's box is not the pane's, and measuring it
  // would resize the pty to a size nobody is looking at.
  const refit = () => {
    if (disposed || !entry.attached || !host.isConnected || !host.clientWidth || !host.clientHeight) return;
    safely(() => fit.fit());
  };
  refit();
  // The first fit runs before the stylesheet and webfont have had their say,
  // so the height it measures is not the final one. Measure again once the
  // browser has laid the pane out for real.
  requestAnimationFrame(refit);
  void document.fonts?.ready.then(refit).catch(() => {});

  /**
   * Makes the CLI redraw its screen. It runs fullscreen, i.e. on the
   * alternate screen, and only paints on input or on SIGWINCH — so anything
   * that leaves this terminal holding a stale or empty frame has to ask for
   * a repaint rather than wait for one. Resizing to a different size and
   * straight back is what forces it.
   */
  const nudgeRepaint = () => {
    if (disposed) return;
    safely(() => term.refresh(0, term.rows - 1));
    sendResize(term.cols, Math.max(1, term.rows - 1));
    clearTimeout(repaintTimer);
    repaintTimer = setTimeout(() => {
      if (disposed) return;
      // Re-read the size instead of restoring the one captured above: the
      // layout can settle within these 60ms (the metadata row rewrapping is
      // enough), and sending a stale row count leaves the pty believing the
      // viewport is taller than it is — which pushes the CLI's input box
      // below the bottom of the pane until the pane is reopened.
      refit();
      sendResize(term.cols, term.rows);
    }, 60);
  };

  let webgl: WebglAddon | null = null;
  let ctxLossSub: { dispose(): void } | null = null;
  // Dispose the addon through its own dispose(): loadAddon replaced that
  // method with the AddonManager's wrapper, which is idempotent and marks the
  // addon disposed so term.dispose() skips it later. The previous fix here
  // bound the wrapper, then swapped in a no-op — which worked, but only by
  // accident of the wrapper's own bookkeeping, and it hid the real story:
  // the addon's genuine first dispose throws a TypeError from deep inside
  // its listener teardown (upstream @xterm/addon-webgl, after the renderer
  // and its GL context are already released). That known-benign throw was
  // being logged as a teardown warning on every single pane close — 2k+
  // lines a day. Drop our own context-loss subscription first (it is the
  // teardown path the throw walks through), and if the addon still throws a
  // bare TypeError, swallow it; anything else is a real failure and is kept.
  const disposeWebgl = () => {
    const addon = webgl;
    webgl = null;
    if (!addon) return;
    safely(() => ctxLossSub?.dispose());
    ctxLossSub = null;
    try {
      addon.dispose();
    } catch (e) {
      if (!(e instanceof TypeError)) {
        logWarn('terminal', `webgl dispose failed: ${(e as Error)?.stack ?? String(e)}`);
      }
    }
  };
  try {
    webgl = new WebglAddon();
    ctxLossSub = webgl.onContextLoss(() => {
      // Chromium force-loses the oldest context once too many are live, and
      // the pane that owns it is usually still on screen. Falling back to the
      // DOM renderer silently would leave it blank until the next keystroke.
      disposeWebgl();
      nudgeRepaint();
    });
    term.loadAddon(webgl);
  } catch {
    webgl = null; // canvas/DOM renderer fallback
  }

  const mo = new MutationObserver(() => {
    safely(() => {
      term.options.theme = themeFor(dark());
    });
  });
  const appEl = document.querySelector('[data-app]');
  if (appEl) mo.observe(appEl, { attributes: true, attributeFilter: ['data-theme'] });

  const ro = new ResizeObserver(refit);
  ro.observe(host);

  entry.wake = () => {
    if (disposed) return;
    refit();
    nudgeRepaint();
  };

  entry.dispose = () => {
    disposed = true;
    clearTimeout(repaintTimer);
    safely(() => ro.disconnect());
    safely(() => mo.disconnect());
    safely(() => bufferSub.dispose());
    unlisteners.forEach(u => safely(u));
    // Drop the webgl context before the terminal: disposing it afterwards
    // hits already-freed renderer state and throws.
    disposeWebgl();
    safely(() => term.dispose());
    host.remove();
  };

  if (!accountPath) {
    term.write(
      `\r\n\x1b[33mAccount "${chat.account}" is not on disk.\x1b[0m\r\n` +
        'Add it in the accounts panel, or delete this chat.\r\n'
    );
    return entry;
  }

  // The title is the first thing typed into the chat, taken as it is typed.
  // The registry route — CLI writes its entry, the watcher polls it a few
  // seconds later — leaves a new chat sitting in the list as "chat 12" while
  // everything around it is named after its prompt, which is a very easy row
  // to lose track of. Reading the keystrokes is a guess (an edited line keeps
  // the characters that were rubbed out), but the watcher replaces it with
  // the transcript's own version as soon as there is one.
  let typed = '';
  let titled = false;
  const dataSub = term.onData(d => {
    if (!titled) {
      const clean = typedOnly(d);
      const enter = clean.indexOf('\r');
      typed += enter < 0 ? clean : clean.slice(0, enter);
      if (typed.length > 400) typed = typed.slice(-400);
      if (enter >= 0) {
        const line = typed.replace(/[\x00-\x1f\x7f]/g, '').trim();
        typed = '';
        const fresh = useChats.getState().findChat(chatId);
        // A slash command is the CLI's business, not a title, and a chat the
        // user has named by hand keeps that name.
        if (line.length > 1 && !line.startsWith('/') && fresh && !fresh.nameCustom) {
          titled = true;
          useChats.getState().setName(chatId, line.slice(0, 80));
        }
      }
    }
    sendWrite(d);
  });
  const resizeSub = term.onResize(({ cols, rows }) => {
    if (disposed) return;
    sendResize(cols, rows);
  });
  unlisteners.push(() => dataSub.dispose(), () => resizeSub.dispose());

  void (async () => {
    const un1 = await onPtyOutput(p => {
      if (p.id === chatId) term.write(p.data);
    });
    const un2 = await onPtyExit(p => {
      if (p.id === chatId) {
        sessionLive = false;
        useChats.getState().setStatus(chatId, 'resting');
        term.write('\r\n\x1b[2m[session exited]\x1b[0m\r\n');
      }
    });
    // The dispose may already have run while these were still in flight.
    if (disposed) {
      safely(un1);
      safely(un2);
      return;
    }
    unlisteners.push(un1, un2);

    // Resume a known session after an app restart; if it lived in a
    // worktree, relaunch from that worktree instead of creating a new one.
    let backlog: string;
    try {
      backlog = await ensureSession({
        chatId,
        folder: chat.worktreePath || folderPath,
        accountPath,
        model: chat.model,
        effort: chat.effort,
        perm: chat.perm,
        worktree: chat.worktree && !chat.worktreePath,
        resume: chat.sessionId ?? null
      });
    } catch {
      // The ipc layer already logged the cause. Without this catch the
      // failure escaped the async block as an unhandled rejection and the
      // pane just sat empty with no explanation.
      if (!disposed) {
        useChats.getState().setStatus(chatId, 'resting');
        term.write('\r\n\x1b[31m[failed to start session]\x1b[0m\r\n');
      }
      return;
    }
    if (disposed) return;
    sessionLive = true;

    if (!backlog) {
      // Fresh session: the pty starts at a placeholder size, so this is also
      // the first real SIGWINCH and the CLI paints itself.
      sendResize(term.cols, term.rows);
      // Nothing spawns a session in a browser, so the README fixture paints
      // its own. Dropped from a release build along with the module.
      if (import.meta.env.DEV) void import('../../dev/demo').then(m => m.paint(term, chatId));
      return;
    }

    // Reattaching to a live session. The replayed bytes are a tail: the
    // escape that entered the alternate screen sits at the very start of the
    // session and has long since been trimmed by the scrollback cap, so
    // replaying leaves the terminal on the normal screen with the CLI's UI
    // scattered through its history. Wipe that and ask for a real repaint.
    //
    // The two steps have to be ordered explicitly. term.write() only queues
    // the bytes — xterm parses them on its own schedule — while reset() runs
    // there and then, so writing and resetting back to back usually reset an
    // empty terminal and then painted the stale tail over the top of it, and
    // the repaint went out before any of it landed. That is what left panes
    // showing a bare prompt box with the conversation missing until the next
    // keystroke. The callback is the only ordering guarantee xterm offers.
    term.write(backlog, () => {
      if (disposed) return;
      term.reset();
      nudgeRepaint();
    });
  })();

  return entry;
}

/** Disposes the terminals nobody is showing, oldest put-down first. */
function sweep() {
  const idle = [...live.values()]
    .filter(e => e.idleAt != null)
    .sort((a, b) => (a.idleAt ?? 0) - (b.idleAt ?? 0));
  for (const e of idle.slice(0, Math.max(0, idle.length - WARM))) {
    live.delete(e.chatId);
    e.dispose();
  }
}

/**
 * Not before the rest of the render has had its say. Changing layout takes the
 * whole board down and builds it again, so every terminal on screen is let go
 * and then asked for right back — and a sweep run in the middle of that would
 * throw away the one terminal the user is looking at, which is the single most
 * expensive thing it could pick. React puts every cleanup and every effect of
 * a commit in one synchronous pass, so a microtask lands after the dust.
 */
let sweepQueued = false;
function sweepSoon() {
  if (sweepQueued) return;
  sweepQueued = true;
  queueMicrotask(() => {
    sweepQueued = false;
    sweep();
  });
}

/**
 * The terminal for this chat, made if there isn't one and woken if there is.
 * The caller owns where `host` goes and must call `release` when it stops
 * showing it.
 */
export function acquire(spec: TermSpec): TermHandle {
  const found = live.get(spec.chat.id);
  // An account that arrived late — or changed — means the session was spawned
  // against the wrong config, or not at all. That terminal is not reusable.
  if (found && found.accountPath === spec.accountPath) {
    found.idleAt = null;
    found.attached = true;
    return found;
  }
  if (found) {
    live.delete(found.chatId);
    found.dispose();
  }
  const entry = create(spec);
  entry.idleAt = null;
  entry.attached = true;
  live.set(spec.chat.id, entry);
  return entry;
}

/**
 * Puts the terminal back in the lot. It keeps running, and may be disposed
 * once too many are waiting. `from` is the box the caller had it in: a pane
 * that has already handed the terminal on to another one must not park it out
 * from under its new host.
 */
export function release(chatId: string, from?: HTMLElement) {
  const entry = live.get(chatId);
  if (!entry) return;
  if (from && entry.host.parentElement !== from) return;
  entry.attached = false;
  entry.idleAt = ++releases;
  parkingLot().appendChild(entry.host);
  sweepSoon();
}

/** Nothing to come back to: the chat is gone, so its terminal goes with it. */
export function forget(chatId: string) {
  const entry = live.get(chatId);
  if (!entry) return;
  live.delete(chatId);
  entry.dispose();
}
