import { useEffect, useRef } from 'react';
import { Terminal as XTerm } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import { WebglAddon } from '@xterm/addon-webgl';
import '@xterm/xterm/css/xterm.css';
import { notifyWaiting } from '../../shared/lib/notify';
import type { Chat } from '../../shared/types';
import { ensureSession, resizeSession, sessionMeta, writeSession } from '../../ipc/commands';
import { onPtyExit, onPtyOutput } from '../../ipc/events';
import { useChats } from '../chats/chats.store';
import { useAccounts } from '../accounts/accounts.store';

export const dark = () => document.querySelector('[data-app]')?.getAttribute('data-theme') === 'dark';

export const themeFor = (isDark: boolean) =>
  isDark
    ? { background: '#00000000', foreground: '#d8e6e2', cursor: '#d8e6e2', selectionBackground: '#3a5a55' }
    : { background: '#00000000', foreground: '#31413c', cursor: '#31413c', selectionBackground: '#c2dbcd' };

// The CLI registry reports its own status vocabulary; fold it into the
// three-state design palette.
const mapStatus = (s: string | null): Chat['status'] => {
  if (!s) return 'resting';
  if (s === 'busy' || s === 'working') return 'working';
  if (/wait|input|block|attention|permission/.test(s)) return 'waiting';
  return 'resting';
};

export function Terminal({ chat, folderPath }: { chat: Chat; folderPath: string }) {
  const hostRef = useRef<HTMLDivElement>(null);
  const setStatus = useChats(s => s.setStatus);

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

    const accountPath =
      useAccounts.getState().accounts.find(a => a.name === chat.account)?.path ?? '';

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
      unlisteners.push(un1, un2);
      if (disposed) return;

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
      if (backlog) term.write(backlog);
      void resizeSession(chat.id, term.cols, term.rows);
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
          if (meta.name && !fresh.nameCustom) setName(chat.id, meta.name);
          const next = mapStatus(meta.status);
          if (next === 'waiting' && fresh.status !== 'waiting') {
            void notifyWaiting(fresh.name);
          }
          setSt(chat.id, next);
          if (meta.context != null) setContext(chat.id, meta.context);
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
    const resizeSub = term.onResize(({ cols, rows }) => void resizeSession(chat.id, cols, rows));

    const ro = new ResizeObserver(() => fit.fit());
    ro.observe(host);

    const mo = new MutationObserver(() => {
      term.options.theme = themeFor(dark());
    });
    const appEl = document.querySelector('[data-app]');
    if (appEl) mo.observe(appEl, { attributes: true, attributeFilter: ['data-theme'] });

    return () => {
      disposed = true;
      clearInterval(metaTimer);
      ro.disconnect();
      mo.disconnect();
      dataSub.dispose();
      resizeSub.dispose();
      unlisteners.forEach(u => u());
      term.dispose();
    };
    // Session identity is the chat id; the rest is captured at spawn time.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [chat.id]);

  return (
    <div
      style={{
        flex: 1, minHeight: 0, background: 'var(--term)', padding: '12px 14px',
        fontFamily: "'JetBrains Mono', ui-monospace, monospace", fontSize: 11.5,
        color: 'var(--dim)', overflow: 'hidden', display: 'flex', flexDirection: 'column'
      }}
    >
      <div ref={hostRef} style={{ flex: 1, minHeight: 0 }} />
    </div>
  );
}
