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
import '@xterm/xterm/css/xterm.css';
import type { Chat } from '../../shared/types';
import { ensureClaudeSession, ensureCodexSession } from '../../ipc/sessions';
import { onPtyExit, onPtyOutput } from '../../ipc/events';
import { useChats } from '../chats/chats.store';
import { claude } from '../providers';
import { attachClipboardKeys } from './terminal/keys';
import { linkHandler } from './terminal/links';
import { ptySender } from './terminal/pty';
import { safely } from './terminal/safely';
import { SCROLLBACK, pinToBottom, replayTail } from './terminal/scrollback';
import { sizing } from './terminal/sizing';
import { TERM_FONT_FAMILY, TERM_FONT_SIZE, dark, followTheme, themeFor } from './terminal/theme';
import { titleFromTyping } from './terminal/title';
import { loadWebgl } from './terminal/webgl';

/** How many terminals stay alive with no pane showing them. */
const WARM = 3;

/**
 * Starts a chat's CLI where it left off, or if it runs already answers with what
 * it has printed. If it lived in a worktree it relaunches in that worktree instead
 * of creating a new one. A worktree Codex made is started in; it stays until Luna
 * removes it. A worktree Claude Code made is asked for by name from the project
 * folder (its own `--worktree <name> --resume <id>`) rather than by starting inside
 * it: Claude Code removes a clean worktree when its session quits, which Luna's quit
 * and every update make it do, and by name it is put back where it was.
 */
export function startSession(chat: Chat, folderPath: string, accountPath: string): Promise<string> {
  const claudeWorktree =
    chat.provider === 'claude' && chat.worktree ? claude.worktreeName(folderPath, chat.worktreePath) : null;
  return chat.provider === 'codex'
    ? ensureCodexSession({
        chatId: chat.id,
        folder: chat.worktreePath || folderPath,
        worktree: chat.worktree && !chat.worktreePath,
        accountPath,
        model: chat.model,
        effort: chat.effort,
        approval: chat.approval,
        sandbox: chat.sandbox,
        resume: chat.sessionId ?? null,
        tools: chat.tools ?? false,
        hooksUnasked: chat.hooksUnasked ?? false,
        parent: chat.parentId ?? null
      })
    : ensureClaudeSession({
        chatId: chat.id,
        folder: claudeWorktree ? folderPath : chat.worktreePath || folderPath,
        accountPath,
        model: chat.model,
        effort: chat.effort,
        perm: chat.perm,
        worktree: chat.worktree && (!chat.worktreePath || claudeWorktree !== null),
        worktreeName: claudeWorktree,
        resume: chat.sessionId ?? null,
        tools: chat.tools ?? false,
        parent: chat.parentId ?? null,
        readOnly: chat.readOnly ?? false,
        agent: chat.agent ?? null
      });
}

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
  /** It had the keyboard when it was put down: moving a node out of the
   *  document blurs it, so the pane it lands in next hands it back. */
  refocus: boolean;
  /** Its session ended, or never started: kept on screen to say so, but
   *  showing it again has to start one rather than reattach to nothing. */
  dead: boolean;
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
    scrollback: SCROLLBACK,
    // Nudge unreadable theme-vs-content combinations without flattening colors.
    minimumContrastRatio: 3,
    theme: themeFor(dark()),
    linkHandler: linkHandler(chatId)
  });
  const fit = new FitAddon();
  term.loadAddon(fit);
  term.open(host);
  attachClipboardKeys(term);
  const bufferSub = pinToBottom(term);

  let disposed = false;
  const unlisteners: Array<() => void> = [];
  const pty = ptySender(chatId);

  const entry: Entry = {
    chatId,
    accountPath,
    host,
    attached: false,
    idleAt: 0,
    refocus: false,
    dead: false,
    wake: () => {},
    dispose: () => {}
  };

  const size = sizing(term, fit, host, {
    alive: () => !disposed,
    canFit: () => !disposed && entry.attached,
    resize: pty.resize
  });
  const disposeWebgl = loadWebgl(term, size.nudgeRepaint);
  const stopTheme = followTheme(term);

  entry.wake = () => {
    if (disposed) return;
    size.refit();
    size.nudgeRepaint();
    // Only while the keyboard is still nowhere: something the user has
    // focused since keeps it.
    const free = !document.activeElement || document.activeElement === document.body;
    if (entry.refocus && free) host.querySelector<HTMLTextAreaElement>('.xterm-helper-textarea')?.focus();
    entry.refocus = false;
  };

  entry.dispose = () => {
    disposed = true;
    size.dispose();
    safely(stopTheme);
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

  const takeTitle = titleFromTyping(chatId);
  const dataSub = term.onData(d => {
    takeTitle(d);
    pty.write(d);
  });
  const resizeSub = term.onResize(({ cols, rows }) => {
    if (disposed) return;
    pty.resize(cols, rows);
  });
  unlisteners.push(
    () => dataSub.dispose(),
    () => resizeSub.dispose()
  );

  void (async () => {
    const un1 = await onPtyOutput(p => {
      if (p.id === chatId) term.write(p.data);
    });
    const un2 = await onPtyExit(p => {
      if (p.id === chatId) {
        pty.setLive(false);
        entry.dead = true;
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

    let backlog: string;
    try {
      backlog = await startSession(chat, folderPath, accountPath);
    } catch (e) {
      // The ipc layer already logged the cause. Without this catch the
      // failure escaped the async block as an unhandled rejection and the
      // pane just sat empty with no explanation.
      entry.dead = true;
      if (!disposed) {
        useChats.getState().setStatus(chatId, 'resting');
        const why = e instanceof Error ? e.message : String(e);
        term.write(`\r\n\x1b[31m[failed to start session${why ? `: ${why}` : ''}]\x1b[0m\r\n`);
      }
      return;
    }
    if (disposed) return;
    pty.setLive(true);

    if (!backlog) {
      // Fresh session: the pty starts at a placeholder size, so this is also
      // the first real SIGWINCH and the CLI paints itself.
      pty.resize(term.cols, term.rows);
      // Nothing spawns a session in a browser, so the README fixture paints
      // its own. Dropped from a release build along with the module.
      if (import.meta.env.DEV) void import('../../dev/demo/terminal').then(m => m.paint(term, chatId));
      return;
    }

    replayTail(term, chatId, backlog, () => disposed, size.nudgeRepaint);
  })();

  return entry;
}

/** Disposes the terminals nobody is showing, oldest put-down first. */
function sweep() {
  const idle = [...live.values()].filter(e => e.idleAt != null).sort((a, b) => (a.idleAt ?? 0) - (b.idleAt ?? 0));
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
  // against the wrong config, or not at all. That terminal is not reusable,
  // and nor is one whose session has gone.
  if (found && found.accountPath === spec.accountPath && !found.dead) {
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
  entry.refocus = entry.host.contains(document.activeElement);
  parkingLot().appendChild(entry.host);
  sweepSoon();
}

/** Gives this chat's terminal the keyboard, if it is on screen. */
export function focusTerminal(chatId: string) {
  const entry = live.get(chatId);
  if (entry?.attached) entry.host.querySelector<HTMLTextAreaElement>('.xterm-helper-textarea')?.focus();
}

/** Every terminal at once, for when every session behind them has gone —
 *  the core stopped — and each pane has to start its session over. */
export function forgetAll() {
  for (const entry of live.values()) entry.dispose();
  live.clear();
}

/** Nothing to come back to: the chat is gone, so its terminal goes with it. */
export function forget(chatId: string) {
  const entry = live.get(chatId);
  if (!entry) return;
  live.delete(chatId);
  entry.dispose();
}
