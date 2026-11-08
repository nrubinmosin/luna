import type { Folder } from '../../shared/types';
import { tail2 } from '../../shared/lib/format';
import { Chevron, ToolButton } from '../../shared/ui/icons';
import { useChats } from './chats.store';
import { useNewChat } from '../new-chat/newchat.store';
import { ChatRow } from './ChatRow';

/**
 * A folder's chats in sidebar order. Every row is a chat of the person's own:
 * the helpers a chat's agent runs are no rows, only a count on that chat's row
 * (BackgroundChip), and one opened as a chat is the person's like any other.
 */
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
            {folder.chats.map(chat => (
              <ChatRow key={chat.id} chat={chat} />
            ))}
          </div>
        </div>
      </div>
    </div>
  );
}
