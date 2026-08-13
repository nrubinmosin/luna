import { useCallback, useEffect, useState } from 'react';
import type { Folder } from '../../shared/types';
import { tail2, tint } from '../../shared/lib/format';
import { useChats } from './chats.store';
import { useNewChat } from '../new-chat/newchat.store';
import { useAccounts } from '../accounts/accounts.store';
import { ChatRow } from './ChatRow';
import { orphanWorktrees, removeOrphanWorktrees } from '../../ipc/commands';

export function FolderSection({ folder }: { folder: Folder }) {
  const toggleFolder = useChats(s => s.toggleFolder);
  const t = tail2(folder.path);
  const live = folder.chats.filter(c => !c.archived);
  const archived = folder.chats.filter(c => c.archived);
  // Kept out of the store: which archive drawers are open is worth exactly as
  // much as the current window, and persisting it would only mean one more
  // shape to migrate.
  const [showArchived, setShowArchived] = useState(false);

  // Worktrees left behind by crashes or by chats deleted before their path was
  // known. Recheck whenever the folder's chats change — that is when one is
  // most likely to have just been created or dropped.
  const [orphans, setOrphans] = useState<string[]>([]);
  const inUse = folder.chats.map(c => c.worktreePath).filter((p): p is string => !!p);
  const inUseKey = inUse.join('|');

  const rescan = useCallback(() => {
    const accountPaths = useAccounts.getState().accounts.map(a => a.path);
    void orphanWorktrees(folder.path, inUseKey ? inUseKey.split('|') : [], accountPaths)
      .then(setOrphans)
      .catch(() => setOrphans([]));
  }, [folder.path, inUseKey]);

  useEffect(() => {
    rescan();
    const t = setInterval(rescan, 60_000);
    return () => clearInterval(t);
  }, [rescan]);

  const sweep = () => {
    const accountPaths = useAccounts.getState().accounts.map(a => a.path);
    void removeOrphanWorktrees(folder.path, inUseKey ? inUseKey.split('|') : [], accountPaths)
      .then(rescan)
      .catch(rescan);
  };

  return (
    <div className="xp-raised" style={{ background: 'var(--bg)', overflow: 'hidden' }}>
      <div
        onClick={() => toggleFolder(folder.id)}
        title={folder.path}
        className="hover-dim folder-row"
        style={{
          display: 'flex', alignItems: 'center', gap: 6, height: 27, padding: '0 7px', cursor: 'default',
          background: folder.open ? tint(9, 'var(--panel)') : 'var(--panel)',
          borderBottom: `1px solid ${folder.open ? 'var(--line)' : 'transparent'}`
        }}
      >
        <span style={{ fontSize: 'var(--fs-1)', color: 'var(--faint)', width: 8, flex: 'none', transform: folder.open ? 'rotate(90deg)' : 'rotate(0deg)' }}>▶</span>
        <span style={{ fontSize: 'var(--fs-2)', color: 'var(--faint)', flex: 'none', whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis', maxWidth: 70 }}>
          {t.parent}
          <span style={{ opacity: 0.6 }}> / </span>
        </span>
        <span style={{ flex: '1 1 auto', minWidth: 40, fontSize: 'var(--fs-4)', fontWeight: 700, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
          {t.leaf}
        </span>
        <span className="folder-actions" style={{ display: 'flex', alignItems: 'center', gap: 4, flex: 'none' }}>
          {orphans.length > 0 && (
            <span
              onClick={e => {
                e.stopPropagation();
                sweep();
              }}
              title={
                `${orphans.length} stale worktree${orphans.length > 1 ? 's' : ''} no chat is using — ` +
                `click to delete them and their branches:\n` +
                orphans.join('\n')
              }
              className="hover-danger"
              style={{
                fontSize: 'var(--fs-1)', color: 'var(--dim)', flex: 'none', background: 'var(--chip)',
                padding: '1px 6px', borderRadius: 2, cursor: 'default', whiteSpace: 'nowrap'
              }}
            >
              ⌫ {orphans.length}
            </span>
          )}
          <span
            onClick={e => {
              e.stopPropagation();
              useNewChat.getState().openDialog(folder.path);
            }}
            title="New chat in this folder"
            className="hover-bg"
            style={{ width: 17, height: 17, flex: 'none', borderRadius: 2, display: 'grid', placeItems: 'center', fontSize: 'var(--fs-5)', color: 'var(--dim)', cursor: 'default', lineHeight: 1 }}
          >
            +
          </span>
        </span>
        <span style={{ fontSize: 'var(--fs-1)', color: 'var(--faint)', flex: 'none', background: 'var(--chip)', padding: '1px 5px', borderRadius: 2 }}>
          {live.length}
        </span>
      </div>
      {folder.open && (
        <div style={{ display: 'flex', flexDirection: 'column', padding: 4 }}>
          {live.map(c => (
            <ChatRow key={c.id} chat={c} />
          ))}
          {archived.length > 0 && (
            <>
              <div
                onClick={() => setShowArchived(v => !v)}
                className="hover-bg"
                title="Chats hidden from the list. Their sessions are untouched."
                style={{
                  display: 'flex', alignItems: 'center', gap: 6, height: 22, padding: '0 6px',
                  borderRadius: 2, cursor: 'default', fontSize: 'var(--fs-2)', color: 'var(--faint)'
                }}
              >
                <span style={{ width: 8, flex: 'none', fontSize: 'var(--fs-1)', transform: showArchived ? 'rotate(90deg)' : 'none' }}>▶</span>
                <span style={{ flex: 1 }}>Archived</span>
                <span style={{ background: 'var(--chip)', padding: '1px 5px', borderRadius: 2 }}>{archived.length}</span>
              </div>
              {showArchived && archived.map(c => <ChatRow key={c.id} chat={c} />)}
            </>
          )}
        </div>
      )}
    </div>
  );
}
