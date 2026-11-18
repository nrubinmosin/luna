import { memo } from 'react';
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
 * `firstNo` is where its rows start in the numbered run (`numberedChats`).
 * Memoized: a folder the store did not touch is the same object
 * (`foldersOfGroup`), and does not render again.
 */
export const FolderSection = memo(function FolderSection({ folder, firstNo }: { folder: Folder; firstNo: number }) {
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
      {/* Folded, its rows are not there at all: nothing to render while out of sight. */}
      {folder.open && (
        <div className="tp-body" style={{ display: 'flex', flexDirection: 'column', gap: 1 }}>
          {folder.chats.map((chat, i) => (
            <ChatRow key={chat.id} chat={chat} listNo={firstNo + i < 9 ? firstNo + i + 1 : null} />
          ))}
        </div>
      )}
    </div>
  );
});
