import type { Folder } from '../../shared/types';
import { tail2, tint } from '../../shared/lib/format';
import { useChats } from './chats.store';
import { useNewChat } from '../new-chat/newchat.store';
import { ChatRow } from './ChatRow';

export function FolderSection({ folder }: { folder: Folder }) {
  const toggleFolder = useChats(s => s.toggleFolder);
  const t = tail2(folder.path);

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
