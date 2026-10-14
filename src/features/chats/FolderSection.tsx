import { useCallback, useEffect, useState } from 'react';
import type { Chat, Folder } from '../../shared/types';
import { tail2 } from '../../shared/lib/format';
import { Chevron, Icon, ToolButton } from '../../shared/ui/icons';
import { allChats, useChats } from './chats.store';
import { useNewChat } from '../new-chat/newchat.store';
import { useAccounts } from '../accounts/accounts.store';
import { ChatRow } from './ChatRow';
import { orphanWorktrees } from '../../ipc/commands';
import { SweepWorktreesDialog } from './SweepWorktreesDialog';

/**
 * The folder's rows in sidebar order: a chat with no parent, then the chats
 * an agent of its spawned. A child whose parent is not here — deleted, or
 * in another folder — is listed on its own, marked as an orphan (ChatRow
 * tells the two apart). One level:
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

  // Worktrees left behind by crashes, by chats deleted before their path was
  // known, or by chats deleted with the worktree kept. Recheck whenever the
  // folder's chats change — that is when one is most likely to have just been
  // created or dropped.
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

  const [sweeping, setSweeping] = useState(false);

  return (
    <div className="tp-group">
      <div onClick={() => toggleFolder(folder.id)} title={folder.path} className="tp-head">
        <Icon name="folder" />
        <span style={{ flex: '1 1 auto', minWidth: 40, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
          <span style={{ fontWeight: 400, opacity: 0.7 }}>{t.parent} / </span>
          {t.leaf}
        </span>
        {orphans.length > 0 && (
          <ToolButton
            icon="sweep"
            size={22}
            label={orphans.length}
            title={
              `${orphans.length} stale worktree${orphans.length > 1 ? 's' : ''} no chat is using — ` +
              `click to review and delete:\n` +
              orphans.join('\n')
            }
            onClick={() => setSweeping(true)}
          />
        )}
        <ToolButton
          icon="plus"
          size={22}
          title="New chat in this folder"
          onClick={() => useNewChat.getState().openDialog(folder.path)}
        />
        <Chevron open={folder.open} />
      </div>
      {sweeping && (
        <SweepWorktreesDialog
          folder={folder.path}
          orphans={orphans}
          inUse={inUseKey ? inUseKey.split('|') : []}
          accountPaths={useAccounts.getState().accounts.map(a => a.path)}
          onClose={() => {
            setSweeping(false);
            rescan();
          }}
        />
      )}
      {/* Always mounted, folded by its row height: the fold animates both ways. */}
      <div className="tp-fold" data-open={folder.open ? 'yes' : 'no'}>
        <div>
          <div className="tp-body" style={{ display: 'flex', flexDirection: 'column', gap: 1 }}>
            {/* Children folded unless unfolded by hand: an agent can spawn a
                helper per task, and the list would otherwise grow with every one. */}
            {tree(folder.chats).map(({ chat, depth, childCount, orphan }) => (
              <ChatRow key={chat.id} chat={chat} depth={depth} childCount={childCount} orphan={orphan} />
            ))}
          </div>
        </div>
      </div>
    </div>
  );
}
