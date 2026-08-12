import { useEffect, useRef, useState } from 'react';
import { Terminal as XTerm } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import { WebglAddon } from '@xterm/addon-webgl';
import '@xterm/xterm/css/xterm.css';
import { notifyWaiting } from '../../shared/lib/notify';
import { logWarn } from '../../shared/lib/log';
import { attachClipboardImage, attachFiles, filesFrom } from '../../shared/lib/attach';
import { ACCENT, tint } from '../../shared/lib/format';
import type { Chat } from '../../shared/types';
import { ensureSession, resizeSession, sessionMeta, writeSession } from '../../ipc/commands';
import { onPtyExit, onPtyOutput } from '../../ipc/events';
import { useChats } from '../chats/chats.store';
import { useAccounts } from '../accounts/accounts.store';

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
    logWarn('terminal', `teardown step failed: ${String(e)}`);
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

// The CLI registry reports its own status vocabulary; fold it into the
// three-state design palette.
const mapStatus = (s: string | null): Chat['status'] => {
  if (!s) return 'resting';
  if (s === 'busy' || s === 'working') return 'working';
  if (/wait|input|block|attention|permission/.test(s)) return 'waiting';
  return 'resting';
};

/** A drag carrying real files, as opposed to a chat row being dropped into a pane. */
const hasFiles = (dt: DataTransfer | null) => !!dt && Array.from(dt.types).includes('Files');

export function Terminal({ chat, folderPath }: { chat: Chat; folderPath: string }) {
  const hostRef = useRef<HTMLDivElement>(null);
  const setStatus = useChats(s => s.setStatus);
  // Resolved from the store rather than read once at mount: on a cold start the
  // account list arrives after the panes do, and spawning with an empty path
  // drops CLAUDE_CONFIG_DIR — the CLI then boots on the default config and
  // greets every pane with first-run onboarding.
  const accountsLoaded = useAccounts(s => s.loaded);
  const accountPath = useAccounts(s => s.accounts.find(a => a.name === chat.account)?.path ?? null);
  const [dropping, setDropping] = useState(false);
  const [attaching, setAttaching] = useState(0);

  // Clipboard and drag payloads never carry a usable filesystem path in a
  // webview, so copy the bytes into the app's media store and type the
  // resulting absolute path into the prompt.
  const take = (files: File[]) => {
    if (!files.length) return;
    setAttaching(n => n + files.length);
    void attachFiles(chat.id, files).finally(() => setAttaching(n => Math.max(0, n - files.length)));
  };

  /** Ctrl+V with an image on the clipboard; falls through to normal paste otherwise. */
  const takeClipboardImage = () => {
    setAttaching(n => n + 1);
    void attachClipboardImage(chat.id).finally(() => setAttaching(n => Math.max(0, n - 1)));
  };

  useEffect(() => {
    const host = hostRef.current;
    if (!host || !accountsLoaded) return;

    const term = new XTerm({
      fontFamily: TERM_FONT_FAMILY,
      fontSize: TERM_FONT_SIZE,
      lineHeight: 1.35,
      allowTransparency: false,
      cursorBlink: true,
      // Nudge unreadable theme-vs-content combinations without flattening colours.
      minimumContrastRatio: 3,
      theme: themeFor(dark())
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(host);
    let webgl: WebglAddon | null = null;
    try {
      webgl = new WebglAddon();
      webgl.onContextLoss(() => safely(() => webgl?.dispose()));
      term.loadAddon(webgl);
    } catch {
      webgl = null; // canvas/DOM renderer fallback
    }

    let disposed = false;
    let repaintTimer: ReturnType<typeof setTimeout> | undefined;
    const unlisteners: Array<() => void> = [];
    // Fitting a detached or zero-sized host throws; that happens on every
    // pane close and used to take the whole window down with it.
    const refit = () => {
      if (disposed || !host.isConnected || !host.clientWidth || !host.clientHeight) return;
      safely(() => fit.fit());
    };
    refit();

    if (!accountPath) {
      term.write(
        `\r\n\x1b[33mAccount "${chat.account}" is not on disk.\x1b[0m\r\n` +
          'Add it in the accounts panel, or delete this chat.\r\n'
      );
      return () => {
        safely(() => webgl?.dispose());
        safely(() => term.dispose());
      };
    }

    void (async () => {
      const un1 = await onPtyOutput(p => {
        if (p.id === chat.id) term.write(p.data);
      });
      const un2 = await onPtyExit(p => {
        if (p.id === chat.id) {
          setStatus(chat.id, 'resting');
          term.write('\r\n\x1b[2m[session exited]\x1b[0m\r\n');
        }
      });
      // The cleanup may already have run while these were still in flight.
      if (disposed) {
        safely(un1);
        safely(un2);
        return;
      }
      unlisteners.push(un1, un2);

      // Resume a known session after an app restart; if it lived in a
      // worktree, relaunch from that worktree instead of creating a new one.
      const backlog = await ensureSession({
        chatId: chat.id,
        folder: chat.worktreePath || folderPath,
        accountPath,
        model: chat.model,
        effort: chat.effort,
        perm: chat.perm,
        worktree: chat.worktree && !chat.worktreePath,
        resume: chat.sessionId ?? null
      });
      if (disposed) return;

      if (!backlog) {
        // Fresh session: the pty starts at a placeholder size, so this is also
        // the first real SIGWINCH and the CLI paints itself.
        void resizeSession(chat.id, term.cols, term.rows);
        return;
      }

      // Reattaching to a live session. The CLI runs in fullscreen mode, i.e. on
      // the alternate screen, and only repaints on input or on SIGWINCH.
      // Replaying its scrollback into a fresh xterm therefore leaves whatever
      // the buffer happened to end on — often just the prompt box, with the
      // conversation above it missing until you type. Resizing to a different
      // size and straight back forces a full repaint.
      term.write(backlog);
      const { cols, rows } = term;
      void resizeSession(chat.id, cols, Math.max(1, rows - 1));
      repaintTimer = setTimeout(() => {
        if (!disposed) void resizeSession(chat.id, cols, rows);
      }, 60);
    })();

    // Pick up the CLI's own session registry: AI-derived name + real status.
    const metaTimer = setInterval(() => {
      void sessionMeta(chat.id, accountPath)
        .then(meta => {
          if (!meta) return;
          const { setName, setStatus: setSt, setWorktreePath, setSessionId, setContext, findChat } =
            useChats.getState();
          const fresh = findChat(chat.id);
          if (!fresh) return;
          // The registry name is `derived` in practice — the cwd folder, which
          // for a worktree run is a random codename. Prefer the CLI's own title
          // when it ever produces one, else the session's opening prompt.
          const titled = meta.nameSource === 'auto' || meta.nameSource === 'user';
          const title = (titled && meta.name) || meta.firstPrompt;
          if (title && !fresh.nameCustom) setName(chat.id, title);
          const next = mapStatus(meta.status);
          if (next === 'waiting' && fresh.status !== 'waiting') {
            void notifyWaiting(fresh.name);
          }
          setSt(chat.id, next);
          if (meta.context != null) {
            setContext(chat.id, meta.context, meta.contextTokens ?? null, meta.contextWindow ?? null);
          }
          // Remember where --worktree actually put the session, for cleanup on delete.
          if (meta.cwd && /[\\/]\.claude[\\/]worktrees[\\/]/i.test(meta.cwd)) {
            setWorktreePath(chat.id, meta.cwd);
          }
          // Remember the CLI session id so an app restart can --resume it.
          if (meta.sessionId) setSessionId(chat.id, meta.sessionId);
        })
        .catch(() => {});
    }, 4000);

    const dataSub = term.onData(d => void writeSession(chat.id, d));
    const resizeSub = term.onResize(({ cols, rows }) => {
      if (disposed || cols < 1 || rows < 1) return;
      void resizeSession(chat.id, cols, rows);
    });

    const ro = new ResizeObserver(refit);
    ro.observe(host);

    const mo = new MutationObserver(() => {
      safely(() => {
        term.options.theme = themeFor(dark());
      });
    });
    const appEl = document.querySelector('[data-app]');
    if (appEl) mo.observe(appEl, { attributes: true, attributeFilter: ['data-theme'] });

    return () => {
      disposed = true;
      clearInterval(metaTimer);
      clearTimeout(repaintTimer);
      safely(() => ro.disconnect());
      safely(() => mo.disconnect());
      safely(() => dataSub.dispose());
      safely(() => resizeSub.dispose());
      unlisteners.forEach(u => safely(u));
      // Drop the webgl context before the terminal: disposing it afterwards
      // hits already-freed renderer state and throws.
      safely(() => webgl?.dispose());
      safely(() => term.dispose());
    };
    // Session identity is the chat id; the rest is captured at spawn time.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [chat.id, accountsLoaded, accountPath]);

  return (
    <div
      onPasteCapture={e => {
        const files = filesFrom(e.clipboardData);
        if (files.length) {
          e.preventDefault();
          e.stopPropagation();
          take(files);
          return;
        }
        // WebView2 does not always expose a clipboard image to web content, and
        // xterm would forward an empty paste. When there is no text either, go
        // ask the OS whether it is holding an image.
        if (!e.clipboardData?.getData('text')) {
          e.preventDefault();
          e.stopPropagation();
          takeClipboardImage();
        }
      }}
      onDragOver={e => {
        if (!hasFiles(e.dataTransfer)) return; // a chat drag — let the pane handle it
        e.preventDefault();
        e.stopPropagation();
        e.dataTransfer.dropEffect = 'copy';
        setDropping(true);
      }}
      onDragLeave={e => {
        if (e.currentTarget.contains(e.relatedTarget as Node | null)) return;
        setDropping(false);
      }}
      onDrop={e => {
        if (!hasFiles(e.dataTransfer)) return;
        e.preventDefault();
        e.stopPropagation();
        setDropping(false);
        take(filesFrom(e.dataTransfer));
      }}
      style={{
        flex: 1, minHeight: 0, background: 'var(--term)', padding: '12px 14px',
        fontFamily: TERM_FONT_FAMILY, fontSize: TERM_FONT_SIZE,
        color: 'var(--dim)', overflow: 'hidden', display: 'flex', flexDirection: 'column',
        position: 'relative'
      }}
    >
      <div ref={hostRef} style={{ flex: 1, minHeight: 0 }} />

      {dropping && (
        <div
          style={{
            position: 'absolute', inset: 6, borderRadius: 9, pointerEvents: 'none',
            border: `1.5px dashed ${ACCENT}`, background: tint(14, 'transparent'),
            display: 'grid', placeItems: 'center', fontSize: 13, fontWeight: 590, color: ACCENT
          }}
        >
          Drop to attach
        </div>
      )}
      {attaching > 0 && (
        <div
          style={{
            position: 'absolute', right: 12, bottom: 10, pointerEvents: 'none',
            padding: '3px 9px', borderRadius: 7, background: 'var(--chip)',
            fontSize: 11.5, color: 'var(--dim)'
          }}
        >
          attaching {attaching} file{attaching > 1 ? 's' : ''}…
        </div>
      )}
    </div>
  );
}
