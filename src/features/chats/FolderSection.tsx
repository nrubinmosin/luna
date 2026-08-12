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
    <div style={{ border: '1px solid var(--line)', borderRadius: 11, background: 'var(--bg)', overflow: 'hidden', boxShadow: '0 1px 1px oklch(.4 .04 160 / .05)' }}>
      <div
        onClick={() => toggleFolder(folder.id)}
        title={folder.path}
        className="hover-dim"
        style={{
          display: 'flex', alignItems: 'center', gap: 7, height: 33, padding: '0 9px', cursor: 'default',
          background: folder.open ? tint(9, 'var(--panel)') : 'var(--panel)',
          borderBottom: `1px solid ${folder.open ? 'var(--line)' : 'transparent'}`
        }}
      >
        <span style={{ fontSize: 10, color: 'var(--faint)', width: 8, flex: 'none', transform: folder.open ? 'rotate(90deg)' : 'rotate(0deg)' }}>▶</span>
        <span style={{ fontSize: 12.5, color: 'var(--faint)', flex: 'none', whiteSpace: 'nowrap' }}>
          {t.parent}
          <span style={{ opacity: 0.6 }}> / </span>
        </span>
        <span style={{ flex: '1 1 auto', minWidth: 40, fontSize: 13.5, fontWeight: 640, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
          {t.leaf}
        </span>
        <span
          onClick={e => {
            e.stopPropagation();
            useNewChat.getState().openDialog(folder.path);
          }}
          title="New chat in this folder"
          className="hover-bg"
          style={{ width: 18, height: 18, flex: 'none', borderRadius: 5, display: 'grid', placeItems: 'center', fontSize: 14.5, color: 'var(--dim)', cursor: 'default', lineHeight: 1 }}
        >
          +
        </span>
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
              fontSize: 11, color: 'var(--dim)', flex: 'none', background: 'var(--chip)',
              padding: '1px 7px', borderRadius: 20, cursor: 'default', whiteSpace: 'nowrap'
            }}
          >
            ⌫ {orphans.length}
          </span>
        )}
        <span style={{ fontSize: 11.5, color: 'var(--faint)', flex: 'none', background: 'var(--chip)', padding: '1px 6px', borderRadius: 20 }}>
          {folder.chats.length}
        </span>
      </div>
      {folder.open && (
        <div style={{ display: 'flex', flexDirection: 'column', padding: 4 }}>
          {folder.chats.map(c => (
            <ChatRow key={c.id} chat={c} />
          ))}
        </div>
      )}
    </div>
  );
}
