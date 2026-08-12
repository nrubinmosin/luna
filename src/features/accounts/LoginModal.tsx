import { useEffect, useRef } from 'react';
import { Terminal as XTerm } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import { WebglAddon } from '@xterm/addon-webgl';
import '@xterm/xterm/css/xterm.css';
import type { Account } from '../../shared/types';
import { ensureSession, killSession, resizeSession, writeSession } from '../../ipc/commands';
import { onPtyExit, onPtyOutput } from '../../ipc/events';
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
    try {
      webgl = new WebglAddon();
      webgl.onContextLoss(() => safely(() => webgl?.dispose()));
      term.loadAddon(webgl);
    } catch {
      webgl = null; // canvas/DOM renderer fallback
    }

    let disposed = false;
    const unlisteners: Array<() => void> = [];
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
        if (p.id === id) term.write('\r\n\x1b[2m[session exited — you can close this window]\x1b[0m\r\n');
      });
      if (disposed) {
        safely(un1);
        safely(un2);
        return;
      }
      unlisteners.push(un1, un2);

      const backlog = await ensureSession({
        chatId: id,
        folder: account.path,
        accountPath: account.path,
        model: 'Sonnet',
        effort: 'medium',
        perm: 'Ask',
        worktree: false
      });
      if (disposed) return;
      if (backlog) term.write(backlog);
      void resizeSession(id, term.cols, term.rows);
    })();

    const dataSub = term.onData(d => void writeSession(id, d));
    const resizeSub = term.onResize(({ cols, rows }) => {
      if (disposed || cols < 1 || rows < 1) return;
      void resizeSession(id, cols, rows);
    });
    const ro = new ResizeObserver(refit);
    ro.observe(host);

    return () => {
      disposed = true;
      safely(() => ro.disconnect());
      safely(() => dataSub.dispose());
      safely(() => resizeSub.dispose());
      unlisteners.forEach(u => safely(u));
      safely(() => webgl?.dispose());
      safely(() => term.dispose());
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [id]);

  const close = () => {
    void killSession(id);
    useAccounts.getState().setLoginFor(null);
    void useAccounts.getState().refresh();
  };

  return (
    <div style={{ position: 'fixed', inset: 0, background: 'rgba(0,0,0,.32)', backdropFilter: 'blur(1px)', display: 'grid', placeItems: 'center', zIndex: 70 }}>
      <div className="xp-raised" style={{ width: 720, height: 500, background: 'var(--bg)', borderRadius: 3, boxShadow: 'var(--shadow), var(--border-raised-outer), var(--border-raised-inner)', overflow: 'hidden', display: 'flex', flexDirection: 'column' }}>
        <div className="xp-titlebar" style={{ flex: 'none', display: 'flex', alignItems: 'center', gap: 8, height: 27, padding: '0 4px 0 9px', fontSize: 12.5, fontWeight: 700 }}>
          <span>Sign in — {account.name}</span>
          <span style={{ fontSize: 11, color: 'rgba(255,255,255,.75)', overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap', textShadow: 'none' }}>
            {account.path}
          </span>
          <span style={{ flex: 1 }} />
          <span
            onClick={close}
            className="hover-bg"
            style={{ width: 18, height: 18, borderRadius: 2, display: 'grid', placeItems: 'center', fontSize: 11, color: '#fff', cursor: 'default' }}
          >
            ✕
          </span>
        </div>
        <div style={{ flex: 1, minHeight: 0, background: 'var(--term)', padding: '10px 12px' }}>
          <div ref={hostRef} style={{ width: '100%', height: '100%' }} />
        </div>
      </div>
    </div>
  );
}
