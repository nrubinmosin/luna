import { useState } from 'react';
import type { Chat } from '../../shared/types';
import { STATUS } from '../../shared/ui/status';
import { tint } from '../../shared/lib/format';
import { useChats } from './chats.store';
import { usePanes } from '../panes/panes.store';
import { clearMedia, killSession, removeWorktree } from '../../ipc/commands';

export function ChatRow({ chat }: { chat: Chat }) {
  const active = useChats(s => s.active === chat.id);
  const deleteChat = useChats(s => s.deleteChat);
  const setActive = useChats(s => s.setActive);
  const paneIndex = usePanes(s => s.panes.indexOf(chat.id));
  const dragging = usePanes(s => s.drag === chat.id);
  const { setDrag, setOver, evictChat } = usePanes.getState();
  const st = STATUS[chat.status];
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState('');

  const commitRename = () => {
    setEditing(false);
    const name = draft.trim();
    if (name && name !== chat.name) useChats.getState().renameChat(chat.id, name);
  };

  return (
    <div
      draggable
      onDragStart={e => {
        e.dataTransfer.setData('text/plain', chat.id);
        e.dataTransfer.effectAllowed = 'move';
        setDrag(chat.id);
        setActive(chat.id);
      }}
      onDragEnd={() => {
        setDrag(null);
        setOver(-1);
      }}
      onClick={() => setActive(chat.id)}
      title="Drag into a pane"
      className="hover-bg"
      style={{
        display: 'flex', alignItems: 'center', gap: 8, height: 34, padding: '0 8px',
        borderRadius: 7, cursor: 'grab',
        background: active ? tint(18, 'transparent') : 'transparent',
        opacity: dragging ? 0.45 : 1
      }}
    >
      <span style={{ width: 7, height: 7, borderRadius: '50%', flex: 'none', background: st.color, animation: st.anim }} />
      {editing ? (
        <input
          autoFocus
          value={draft}
          onChange={e => setDraft(e.target.value)}
          onBlur={commitRename}
          onKeyDown={e => {
            if (e.key === 'Enter') commitRename();
            if (e.key === 'Escape') setEditing(false);
          }}
          onClick={e => e.stopPropagation()}
          style={{
            flex: '1 1 auto', minWidth: 40, height: 22, padding: '0 4px', border: '1px solid var(--line)',
            borderRadius: 5, background: 'var(--panel)', color: 'var(--fg)', font: 'inherit', fontSize: 13.5, outline: 'none'
          }}
        />
      ) : (
        <span
          onDoubleClick={e => {
            e.stopPropagation();
            setDraft(chat.name);
            setEditing(true);
          }}
          title="Double-click to rename"
          style={{ flex: '1 1 auto', minWidth: 40, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap', fontSize: 13.5, fontWeight: 500 }}
        >
          {chat.name}
        </span>
      )}
      <span style={{ fontSize: 11.5, color: 'var(--faint)', flex: 'none', whiteSpace: 'nowrap' }}>{st.label}</span>
      <span style={{ fontSize: 10, color: 'var(--accent)', flex: 'none', opacity: paneIndex >= 0 ? 1 : 0 }}>
        ◱{paneIndex >= 0 ? paneIndex + 1 : ''}
      </span>
      <span
        onClick={e => {
          e.stopPropagation();
          const folder = useChats.getState().folderOf(chat.id);
          const wt = chat.worktreePath;
          void killSession(chat.id).then(() => {
            // remove_worktree retries while the killed process releases locks
            if (folder && wt) removeWorktree(folder.path, wt).catch(() => {});
            clearMedia(chat.id).catch(() => {});
          });
          evictChat(chat.id);
          deleteChat(chat.id);
        }}
        title="Delete chat (removes its worktree)"
        className="hover-danger"
        style={{
          width: 18, height: 18, flex: 'none', borderRadius: 5, display: 'grid',
          placeItems: 'center', fontSize: 12, color: 'var(--faint)', cursor: 'default'
        }}
      >
        ✕
      </span>
    </div>
  );
}
