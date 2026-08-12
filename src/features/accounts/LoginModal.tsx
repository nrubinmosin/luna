import { useEffect, useRef } from 'react';
import { Terminal as XTerm } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import { WebglAddon } from '@xterm/addon-webgl';
import '@xterm/xterm/css/xterm.css';
import type { Account } from '../../shared/types';
import { ensureSession, killSession, resizeSession, writeSession } from '../../ipc/commands';
import { onPtyExit, onPtyOutput } from '../../ipc/events';
import { dark, themeFor } from '../panes/Terminal';
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
      fontFamily: "'JetBrains Mono', ui-monospace, monospace",
      fontSize: 11.5,
      lineHeight: 1.25,
      allowTransparency: true,
      cursorBlink: true,
      theme: themeFor(dark())
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(host);
    try {
      const webgl = new WebglAddon();
      webgl.onContextLoss(() => webgl.dispose());
      term.loadAddon(webgl);
    } catch {
      // canvas/DOM renderer fallback
    }
    fit.fit();

    let disposed = false;
    const unlisteners: Array<() => void> = [];

    void (async () => {
      const un1 = await onPtyOutput(p => {
        if (p.id === id) term.write(p.data);
      });
      const un2 = await onPtyExit(p => {
        if (p.id === id) term.write('\r\n\x1b[2m[session exited — you can close this window]\x1b[0m\r\n');
      });
      unlisteners.push(un1, un2);
      if (disposed) return;

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
    const resizeSub = term.onResize(({ cols, rows }) => void resizeSession(id, cols, rows));
    const ro = new ResizeObserver(() => fit.fit());
    ro.observe(host);

    return () => {
      disposed = true;
      ro.disconnect();
      dataSub.dispose();
      resizeSub.dispose();
      unlisteners.forEach(u => u());
      term.dispose();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [id]);

  const close = () => {
    void killSession(id);
    useAccounts.getState().setLoginFor(null);
    void useAccounts.getState().refresh();
  };

  return (
    <div style={{ position: 'fixed', inset: 0, background: 'oklch(.2 .03 160 / .3)', backdropFilter: 'blur(2px)', display: 'grid', placeItems: 'center', zIndex: 70 }}>
      <div style={{ width: 640, height: 440, background: 'var(--bg)', border: '1px solid var(--line)', borderRadius: 14, boxShadow: 'var(--shadow)', overflow: 'hidden', display: 'flex', flexDirection: 'column' }}>
        <div style={{ flex: 'none', display: 'flex', alignItems: 'center', gap: 8, padding: '10px 14px', borderBottom: '1px solid var(--line)', background: 'var(--panel)' }}>
          <div style={{ fontSize: 13, fontWeight: 640 }}>Sign in — {account.name}</div>
          <span style={{ fontSize: 11, color: 'var(--faint)', overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
            {account.path}
          </span>
          <span style={{ flex: 1 }} />
          <span
            onClick={close}
            className="hover-bg"
            style={{ width: 20, height: 20, borderRadius: 5, display: 'grid', placeItems: 'center', fontSize: 12, color: 'var(--dim)', cursor: 'default' }}
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
