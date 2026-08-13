import { useCallback, useEffect, useState } from 'react';
import { killSession, orphanSessions, type OrphanSessionDto } from '../../ipc/commands';
import { tail2 } from '../../shared/lib/format';
import { ConfirmDialog } from '../../shared/ui/ConfirmDialog';
import { usePanes } from '../panes/panes.store';
import { useChats } from './chats.store';

const EVERY_MS = 30_000;

/** `<repo>/.claude/worktrees/<name>` is a session's worktree, not its project. */
const WORKTREE = /^(.*)[\\/]\.claude[\\/]worktrees[\\/][^\\/]+$/;

/**
 * Sessions that are still running with no chat row to reach them through.
 *
 * The app used to have no answer for this: a chat archived by a misclick, or
 * lost to a half-written state restore, left its session burning an account's
 * tokens with no way to see it, stop it or talk to it — the only route back was
 * a terminal and `claude --resume`.
 */
export function OrphanSessions() {
  const folders = useChats(s => s.folders);
  const [found, setFound] = useState<OrphanSessionDto[]>([]);
  const [open, setOpen] = useState(false);
  const [killing, setKilling] = useState<OrphanSessionDto | null>(null);

  const knownKey = folders.flatMap(f => f.chats.map(c => c.id)).sort().join('|');

  const rescan = useCallback(() => {
    const known = knownKey ? knownKey.split('|') : [];
    void orphanSessions(known)
      .then(setFound)
      .catch(() => setFound([]));
  }, [knownKey]);

  useEffect(() => {
    rescan();
    const t = setInterval(rescan, EVERY_MS);
    return () => clearInterval(t);
  }, [rescan]);

  // Nothing to adopt closes the dialog: the last row can go away because the
  // session exited on its own.
  useEffect(() => {
    if (!found.length) setOpen(false);
  }, [found.length]);

  const adopt = (o: OrphanSessionDto) => {
    const m = WORKTREE.exec(o.cwd);
    const folder = m ? m[1] : o.cwd;
    const account = o.accountPath.split(/[\\/]/).filter(Boolean).pop() ?? '';
    // The row is built with the session's own id, which is what reattaches the
    // running pty — backlog included — the moment a pane mounts it. Model,
    // effort and permission mode are the session's own business and cannot be
    // read back from it; these are labels for a row that already exists.
    useChats.getState().addChat(folder, {
      id: o.id,
      name: o.title?.slice(0, 80) || `recovered session ${o.id.slice(-4)}`,
      status: 'resting',
      model: 'Opus',
      effort: 'medium',
      perm: 'Bypass',
      context: 0,
      account,
      // Into the group that is on screen: that is where the person doing the
      // adopting is looking, and a chat dropped into a parked group would just
      // go missing a second time.
      group: usePanes.getState().group,
      worktree: !!m,
      worktreePath: m ? o.cwd : null
    });
    usePanes.getState().autoPlace(o.id);
    rescan();
  };

  const kill = (o: OrphanSessionDto) => {
    setKilling(null);
    void killSession(o.id).finally(rescan);
  };

  if (!found.length) return null;

  return (
    <>
      <div
        onClick={() => setOpen(true)}
        title="Sessions still running with no chat to reach them through"
        className="hover-bg"
        style={{
          margin: '0 8px 6px', padding: '4px 7px', display: 'flex', alignItems: 'center', gap: 6,
          fontSize: 'var(--fs-2)', color: 'var(--fg)', background: 'var(--chip)', cursor: 'default',
          boxShadow: 'var(--border-sunken-outer), var(--border-sunken-inner)'
        }}
      >
        <span style={{ color: '#c0392b', fontWeight: 700 }}>!</span>
        <span style={{ flex: 1 }}>
          {found.length} session{found.length > 1 ? 's' : ''} with no chat
        </span>
        <span style={{ color: 'var(--faint)' }}>show</span>
      </div>

      {open && (
        <div style={{ position: 'fixed', inset: 0, background: 'rgba(0,0,0,.32)', display: 'grid', placeItems: 'center', zIndex: 75 }}>
          <div className="window" style={{ width: 560, boxShadow: 'var(--shadow), var(--border-window-outer), var(--border-window-inner)' }}>
            <div className="title-bar">
              <div className="title-bar-text">Sessions with no chat</div>
              <div className="title-bar-controls">
                <button aria-label="Close" onClick={() => setOpen(false)} />
              </div>
            </div>
            <div className="window-body">
              <div style={{ fontSize: 'var(--fs-3)', color: 'var(--dim)', marginBottom: 10, lineHeight: 1.45 }}>
                These are running right now and answering to nobody. Adopting one puts it back in
                the list with its scrollback; killing one ends it, and its transcript stays on disk
                either way.
              </div>
              <div style={{ display: 'flex', flexDirection: 'column', gap: 6, maxHeight: 320, overflowY: 'auto' }}>
                {found.map(o => {
                  const t = tail2(o.cwd);
                  return (
                    <div
                      key={o.id}
                      style={{
                        display: 'flex', alignItems: 'center', gap: 8, padding: '6px 8px',
                        background: 'var(--bg)', boxShadow: 'var(--border-sunken-outer)'
                      }}
                    >
                      <div style={{ flex: 1, minWidth: 0 }}>
                        <div style={{ fontSize: 'var(--fs-4)', fontWeight: 600, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
                          {o.title || `session ${o.id.slice(-6)}`}
                        </div>
                        <div
                          title={o.cwd}
                          style={{ fontSize: 'var(--fs-1)', color: 'var(--faint)', overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}
                        >
                          {t.parent} / {t.leaf}
                          {o.status ? ` · ${o.status}` : ''}
                          {o.pid ? ` · pid ${o.pid}` : ''}
                        </div>
                      </div>
                      <button className="slim primary" onClick={() => adopt(o)}>
                        Adopt
                      </button>
                      <button className="slim" onClick={() => setKilling(o)}>
                        Kill
                      </button>
                    </div>
                  );
                })}
              </div>
            </div>
          </div>
        </div>
      )}

      {killing && (
        <ConfirmDialog
          title="Kill this session?"
          confirmLabel="Kill"
          body={
            <>
              <b>{killing.title || killing.id}</b> is running right now, and may be part-way through
              something. Its transcript stays on disk, so it can be resumed from the CLI later.
            </>
          }
          onConfirm={() => kill(killing)}
          onCancel={() => setKilling(null)}
        />
      )}
    </>
  );
}
