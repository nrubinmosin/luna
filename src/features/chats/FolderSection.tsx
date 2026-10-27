import type { Chat, Folder } from '../../shared/types';
import { tail2 } from '../../shared/lib/format';
import { Chevron, ToolButton } from '../../shared/ui/icons';
import { useChats } from './chats.store';
import { useNewChat } from '../new-chat/newchat.store';
import { ChatRow } from './ChatRow';

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

  return (
    <div className="tp-group">
      <div onClick={() => toggleFolder(folder.id)} title={folder.path} className="tp-head">
        <span className="tp-name">
          <span className="tp-where">{t.parent}</span>
          <span>{t.leaf}</span>
        </span>
        <ToolButton
          icon="plus"
          size={22}
          title="New chat in this folder"
          onClick={() => useNewChat.getState().openDialog(folder.path)}
        />
        <Chevron open={folder.open} />
      </div>
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
