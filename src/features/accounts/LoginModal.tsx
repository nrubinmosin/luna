import { useEffect, useRef } from 'react';
import { Terminal as XTerm } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import '@xterm/xterm/css/xterm.css';
import type { Account } from '../../shared/types';
import { killSession, resizeSession, writeSession } from '../../ipc/sessions';
import { routePty } from '../panes/terminal/ptyRoutes';
import { attachLoginKeys } from '../panes/terminal/keys';
import { safely } from '../panes/terminal/safely';
import { loadWebgl } from '../panes/terminal/webgl';
import { dark, themeFor, TERM_FONT_FAMILY, TERM_FONT_SIZE } from '../panes/terminal/theme';
import { useAccounts } from './accounts.store';
import { ui } from '../providers';
import { PROVIDER_LABEL } from '../../shared/types';

/** Login sessions a closed window is still ending, by id. The id is the
 *  account's, so a window opened on it again meanwhile would attach to the
 *  very process the kill is on its way to, and hear its output and its exit:
 *  it waits for that end before it listens to the id at all, then starts its
 *  own (the kill takes the session out before it answers). */
const ending = new Map<string, Promise<void>>();
/** How long a window waits for that end before it says it is still waiting
 *  (the kill gives the CLI 10 s to quit, pty/control.rs `GRACE`). It never
 *  starts before that end, however long: a start on the id while the last
 *  one's is still under way would get that very session back from the core,
 *  and the kill would then take it out. */
export const ENDING_WAIT_MS = 15_000;

// The provider's login session inside the account's config dir — a bare
// `claude`, or `codex login` — so credentials land in that folder.
export function LoginModal({ account }: { account: Account }) {
  const hostRef = useRef<HTMLDivElement>(null);
  const id = `login:${account.provider}:${account.name}`;

  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;

    const term = new XTerm({
      fontFamily: TERM_FONT_FAMILY,
      fontSize: TERM_FONT_SIZE,
      lineHeight: 1.35,
      allowTransparency: false,
      cursorBlink: false,
      minimumContrastRatio: 3,
      theme: themeFor(dark())
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(host);
    attachLoginKeys(term);
    const disposeWebgl = loadWebgl(term);

    let disposed = false;
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

    let route: ReturnType<typeof routePty> | null = null;
    // The login session, from the moment it is asked for: closing the window
    // ends it, and one closed before the start has answered is ended once it
    // has (a kill sent sooner finds no session and leaves it running).
    let starting: Promise<string> | null = null;

    void (async () => {
      const before = ending.get(id);
      if (before) {
        term.write('\x1b[2m[waiting for the last sign-in session to end…]\x1b[0m\r\n');
        const timer = setTimeout(() => {
          if (!disposed)
            term.write(
              '\x1b[33m[the last sign-in session has not ended yet; this window starts its own once it has, or close it and try again later]\x1b[0m\r\n'
            );
        }, ENDING_WAIT_MS);
        await before;
        clearTimeout(timer);
        if (disposed) return;
      }
      route = routePty(id, {
        output: data => term.write(data),
        exit: () => {
          sessionLive = false;
          term.write('\r\n\x1b[2m[session exited — you can close this window]\x1b[0m\r\n');
        }
      });
      let backlog: string;
      try {
        await route.ready;
        if (disposed) return;
        starting = ui(account.provider).loginSession(id, account);
        backlog = await starting;
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
      safely(() => route?.unroute());
      disposeWebgl();
      safely(() => term.dispose());
      // Usually already gone by now (login flows exit on their own); a
      // failed kill is not worth an unhandled-rejection entry.
      if (starting) {
        const end = starting.then(
          () => killSession(id).catch(() => {}),
          () => {}
        );
        ending.set(id, end);
        void end.then(() => {
          if (ending.get(id) === end) ending.delete(id);
        });
      }
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [id]);

  // Unmounting ends the session (the effect's cleanup).
  const close = () => {
    useAccounts.getState().setLoginFor(null);
    void useAccounts.getState().refresh();
  };

  return (
    <div
      className="scrim"
      style={{
        position: 'fixed',
        inset: 0,
        background: 'rgba(0,0,0,.32)',
        backdropFilter: 'blur(1px)',
        display: 'grid',
        placeItems: 'center',
        zIndex: 70
      }}
    >
      <div
        className="window"
        style={{
          width: 720,
          height: 500,
          boxShadow: 'var(--shadow), var(--border-window-outer), var(--border-window-inner)'
        }}
      >
        <div className="title-bar" style={{ gap: 8 }}>
          <div className="title-bar-text" style={{ flex: 'none' }}>
            Sign in — {account.name} · {PROVIDER_LABEL[account.provider]}
          </div>
          <div
            style={{
              flex: 1,
              minWidth: 0,
              fontSize: 'var(--fs-2)',
              color: 'rgba(255,255,255,.75)',
              overflow: 'hidden',
              textOverflow: 'ellipsis',
              whiteSpace: 'nowrap'
            }}
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
