import { useEffect, useRef } from 'react';
import { Terminal as XTerm } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import { WebglAddon } from '@xterm/addon-webgl';
import '@xterm/xterm/css/xterm.css';
import type { Account } from '../../shared/types';
import { ensureSession, killSession, resizeSession, writeSession } from '../../ipc/commands';
import { onPtyExit, onPtyOutput } from '../../ipc/events';
import { logWarn } from '../../shared/lib/log';
import { dark, safely, themeFor, TERM_FONT_FAMILY, TERM_FONT_SIZE } from '../panes/Terminal';
import { useAccounts } from './accounts.store';

// Bare `claude` session inside the account's config dir: on a fresh folder it
// walks through the login flow and drops credentials there; on an existing one
// the user can run /login to re-authenticate.
export function LoginModal({ account }: { account: Account }) {
  const hostRef = useRef<HTMLDivElement>(null);
  const id = `login:${account.name}`;

  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;

    const term = new XTerm({
      fontFamily: TERM_FONT_FAMILY,
      fontSize: TERM_FONT_SIZE,
      lineHeight: 1.35,
      allowTransparency: false,
      cursorBlink: true,
      minimumContrastRatio: 3,
      theme: themeFor(dark())
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(host);
    let webgl: WebglAddon | null = null;
    let ctxLossSub: { dispose(): void } | null = null;
    // Same story as the chat terminals (see Terminal.tsx): dispose through the
    // AddonManager's idempotent wrapper so term.dispose() skips the addon, and
    // swallow the known-benign TypeError the addon's own teardown throws after
    // its renderer is already gone.
    const disposeWebgl = () => {
      const addon = webgl;
      webgl = null;
      if (!addon) return;
      safely(() => ctxLossSub?.dispose());
      ctxLossSub = null;
      try {
        addon.dispose();
      } catch (e) {
        // A throw escaping a React effect cleanup takes the root down, so even
        // the unexpected case only gets logged.
        if (!(e instanceof TypeError)) {
          logWarn('terminal', `webgl dispose failed: ${(e as Error)?.stack ?? String(e)}`);
        }
      }
    };
    try {
      webgl = new WebglAddon();
      ctxLossSub = webgl.onContextLoss(disposeWebgl);
      term.loadAddon(webgl);
    } catch {
      webgl = null; // canvas/DOM renderer fallback
    }

    let disposed = false;
    const unlisteners: Array<() => void> = [];
    // Same gate as the chat terminals: the session only exists between
    // ensure_session resolving and pty://exit, and talking to it outside that
    // window logged a "no such session" pair on every login window close.
    let sessionLive = false;
    const sendResize = (cols: number, rows: number) => {
      if (!sessionLive || cols < 1 || rows < 1) return;
      resizeSession(id, cols, rows).catch(() => {});
    };
    const refit = () => {
      if (disposed || !host.isConnected || !host.clientWidth || !host.clientHeight) return;
      safely(() => fit.fit());
    };
    refit();

    void (async () => {
      const un1 = await onPtyOutput(p => {
        if (p.id === id) term.write(p.data);
      });
      const un2 = await onPtyExit(p => {
        if (p.id === id) {
          sessionLive = false;
          term.write('\r\n\x1b[2m[session exited — you can close this window]\x1b[0m\r\n');
        }
      });
      if (disposed) {
        safely(un1);
        safely(un2);
        return;
      }
      unlisteners.push(un1, un2);

      let backlog: string;
      try {
        backlog = await ensureSession({
          chatId: id,
          folder: account.path,
          accountPath: account.path,
          model: 'Sonnet',
          effort: 'medium',
          perm: 'Ask',
          worktree: false
        });
      } catch {
        // Already logged by the ipc layer; keep the rejection out of the
        // global unhandled-promise log and say what happened in the window.
        if (!disposed) term.write('\r\n\x1b[31m[failed to start login session]\x1b[0m\r\n');
        return;
      }
      if (disposed) return;
      sessionLive = true;
      if (backlog) term.write(backlog);
      sendResize(term.cols, term.rows);
    })();

    const dataSub = term.onData(d => {
      if (!sessionLive) return;
      writeSession(id, d).catch(() => {});
    });
    const resizeSub = term.onResize(({ cols, rows }) => {
      if (disposed) return;
      sendResize(cols, rows);
    });
    const ro = new ResizeObserver(refit);
    ro.observe(host);

    return () => {
      disposed = true;
      safely(() => ro.disconnect());
      safely(() => dataSub.dispose());
      safely(() => resizeSub.dispose());
      unlisteners.forEach(u => safely(u));
      disposeWebgl();
      safely(() => term.dispose());
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [id]);

  const close = () => {
    // The session is usually already gone by now (login flows exit on their
    // own); a failed kill is not worth an unhandled-rejection entry.
    killSession(id).catch(() => {});
    useAccounts.getState().setLoginFor(null);
    void useAccounts.getState().refresh();
  };

  return (
    <div style={{ position: 'fixed', inset: 0, background: 'rgba(0,0,0,.32)', backdropFilter: 'blur(1px)', display: 'grid', placeItems: 'center', zIndex: 70 }}>
      <div
        className="window"
        style={{ width: 720, height: 500, boxShadow: 'var(--shadow), var(--border-window-outer), var(--border-window-inner)', display: 'flex', flexDirection: 'column' }}
      >
        <div className="title-bar" style={{ flex: 'none', gap: 8 }}>
          <div className="title-bar-text" style={{ flex: 'none' }}>Sign in — {account.name}</div>
          <div
            style={{ flex: 1, minWidth: 0, fontSize: 'var(--fs-2)', color: 'rgba(255,255,255,.75)', overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}
          >
            {account.path}
          </div>
          <div className="title-bar-controls">
            <button aria-label="Close" onClick={close} />
          </div>
        </div>
        <div style={{ flex: 1, minHeight: 0, background: 'var(--term)', margin: 3, padding: '8px 10px' }}>
          <div ref={hostRef} style={{ width: '100%', height: '100%' }} />
        </div>
      </div>
    </div>
  );
}
