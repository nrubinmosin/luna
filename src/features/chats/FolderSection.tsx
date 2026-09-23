import { useCallback, useEffect, useState } from 'react';
import type { Chat, Folder } from '../../shared/types';
import { tail2, tint } from '../../shared/lib/format';
import { allChats, useChats } from './chats.store';
import { useNewChat } from '../new-chat/newchat.store';
import { useAccounts } from '../accounts/accounts.store';
import { ChatRow } from './ChatRow';
import { orphanWorktrees, removeOrphanWorktrees } from '../../ipc/commands';

/**
 * The folder's rows in sidebar order: a chat with no parent, then the chats
 * an agent of its spawned. A child whose parent is gone — deleted, or in
 * another folder — is listed on its own, marked as an orphan. One level:
 * a grandchild is shown under the child, which sits under its own parent
 * only when both are unfolded.
 */
export interface Row {
  chat: Chat;
  depth: number;
  childCount: number;
  orphan: boolean;
}

export function tree(chats: Chat[]): Row[] {
  const ids = new Set(chats.map(c => c.id));
  const childrenOf = (id: string) => chats.filter(c => c.parentId === id);
  const rows: Row[] = [];
  for (const c of chats) {
    if (c.parentId && ids.has(c.parentId)) continue;
    rows.push({ chat: c, depth: 0, childCount: childrenOf(c.id).length, orphan: !!c.parentId });
    if (c.childrenOpen) rows.push(...descend(c, childrenOf, 1));
  }
  return rows;
}

function descend(parent: Chat, childrenOf: (id: string) => Chat[], depth: number): Row[] {
  const out: Row[] = [];
  for (const c of childrenOf(parent.id)) {
    const kids = childrenOf(c.id);
    out.push({ chat: c, depth, childCount: kids.length, orphan: false });
    if (c.childrenOpen && depth < 4) out.push(...descend(c, childrenOf, depth + 1));
  }
  return out;
}

export function FolderSection({ folder }: { folder: Folder }) {
  const toggleFolder = useChats(s => s.toggleFolder);
  const t = tail2(folder.path);

  // Worktrees left behind by crashes or by chats deleted before their path was
  // known. Recheck whenever the folder's chats change — that is when one is
  // most likely to have just been created or dropped.
  const [orphans, setOrphans] = useState<string[]>([]);
  // Every group's chats, not just the ones listed here: the sidebar shows one
  // group, and sweeping on that view would delete the worktrees belonging to
  // chats parked in the other three.
  const inUseKey = useChats(s =>
    allChats(s.folders)
      .map(c => c.worktreePath)
      .filter((p): p is string => !!p)
      .join('|')
  );

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
      </div>
      {folder.open && (
        <div style={{ display: 'flex', flexDirection: 'column', padding: 4 }}>
          {/* Children folded unless unfolded by hand: an agent can spawn a
              helper per task, and the list would otherwise grow with every one. */}
          {tree(folder.chats).map(({ chat, depth, childCount, orphan }) => (
            <ChatRow key={chat.id} chat={chat} depth={depth} childCount={childCount} orphan={orphan} />
          ))}
        </div>
      )}
    </div>
  );
}
