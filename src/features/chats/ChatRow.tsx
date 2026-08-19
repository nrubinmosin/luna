import { useState } from 'react';
import type { Chat } from '../../shared/types';
import { StatusDot } from '../../shared/ui/StatusDot';
import { chatColorTheme } from '../../shared/ui/chatColors';
import { tint } from '../../shared/lib/format';
import { useChats } from './chats.store';
import { currentLayout, currentSlots, usePanes } from '../panes/panes.store';
import { DeleteChatDialog } from './DeleteChatDialog';

export function ChatRow({ chat }: { chat: Chat }) {
  const active = useChats(s => s.active === chat.id);
  const setActive = useChats(s => s.setActive);
  const dragging = usePanes(s => s.drag === chat.id);
  // Which tile of the board on screen holds this chat, 1-based to match the
  // "pane N of M" wording in an empty pane; null when it isn't on this board.
  const paneNo = usePanes(s => {
    const i = currentSlots(s).indexOf(chat.id);
    return i >= 0 && i < currentLayout(s) ? i + 1 : null;
  });
  const { setDrag, setOver, setSpot } = usePanes.getState();
  const [editing, setEditing] = useState(false);
  const [confirming, setConfirming] = useState(false);
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
      // Fullscreen straight from the list. A chat already on the visible board
      // goes through the pane focus, so Restore lands it back in its tile; one
      // that is not seated anywhere opens as a solo fullscreen and returns to
      // being just a list row. The name span keeps its own double-click
      // (rename) and stops the bubble.
      onDoubleClick={() => {
        const s = usePanes.getState();
        const i = currentSlots(s).indexOf(chat.id);
        if (i >= 0 && i < currentLayout(s)) s.setFocus(i);
        else s.setFocusChat(chat.id);
      }}
      onMouseEnter={() => setSpot(chat.id)}
      onMouseLeave={() => setSpot(null)}
      className="hover-bg chat-row"
      style={{
        display: 'flex', alignItems: 'center', gap: 6, height: 27, padding: '0 6px',
        borderRadius: 2, cursor: 'grab',
        background: active ? tint(22, 'transparent') : 'transparent',
        opacity: dragging ? 0.45 : 1
      }}
    >
      {/* The chat's colour, so a row and the pane wearing the same tint link up
          by eye. In flow whether set or not, same as the mark star below. */}
      <span
        title={chat.color ? 'Chat colour — set from the pane title bar' : undefined}
        style={{
          width: 3, height: 15, borderRadius: 2, flex: 'none',
          background: chatColorTheme(chat.color)?.swatch ?? 'transparent'
        }}
      />
      {/* Kept in flow whether it is set or not — fading it in on hover instead
          of adding it would shift every name in the list on mouse-over. */}
      <span
        onClick={e => {
          e.stopPropagation();
          useChats.getState().toggleMark(chat.id);
        }}
        onDoubleClick={e => e.stopPropagation()}
        title={chat.marked ? 'Marked — click to clear' : 'Mark this chat'}
        className="mark"
        data-on={chat.marked ? 'yes' : 'no'}
      >
        {chat.marked ? '★' : '☆'}
      </span>
      <StatusDot status={chat.status} />
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
          type="text"
          style={{ flex: '1 1 auto', minWidth: 40, height: 'calc(var(--ui) * 1.4)' }}
        />
      ) : (
        <span
          onDoubleClick={e => {
            e.stopPropagation();
            setDraft(chat.name);
            setEditing(true);
          }}
          // The full name first: the row is narrow enough that the ellipsis is
          // the common case, and the rename hint alone hid what was cut off.
          title={`${chat.name}\nDouble-click to rename`}
          style={{ flex: '1 1 auto', minWidth: 40, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap', fontSize: 'var(--fs-4)', fontWeight: 500 }}
        >
          {chat.name}
        </span>
      )}
      {/* Sits after the flexible name, so appearing or vanishing on a layout
          switch only nudges the controls on the right, never the names. */}
      {paneNo != null && (
        <span
          title={`Showing in pane ${paneNo} of the current layout`}
          style={{
            minWidth: 15, height: 15, padding: '0 2px', flex: 'none', borderRadius: 2,
            display: 'grid', placeItems: 'center', fontSize: 'var(--fs-1)', fontWeight: 600,
            fontVariantNumeric: 'tabular-nums', color: 'var(--faint)', border: '1px solid var(--line)'
          }}
        >
          {paneNo}
        </span>
      )}
      <span
        onClick={e => {
          e.stopPropagation();
          useChats.getState().setArchived(chat.id, !chat.archived);
        }}
        onDoubleClick={e => e.stopPropagation()}
        title={chat.archived ? 'Unarchive — put it back in the list' : 'Archive — hide it, keep everything'}
        className="hover-bg"
        style={{
          width: 17, height: 17, flex: 'none', borderRadius: 2, display: 'grid',
          placeItems: 'center', fontSize: 'var(--fs-2)', color: 'var(--faint)', cursor: 'default'
        }}
      >
        {chat.archived ? '↥' : '↧'}
      </span>
      <span
        onClick={e => {
          e.stopPropagation();
          setConfirming(true);
        }}
        onDoubleClick={e => e.stopPropagation()}
        title="Delete chat"
        className="hover-danger"
        style={{
          width: 17, height: 17, flex: 'none', borderRadius: 2, display: 'grid',
          placeItems: 'center', fontSize: 'var(--fs-2)', color: 'var(--faint)', cursor: 'default'
        }}
      >
        ✕
      </span>

      {confirming && <DeleteChatDialog chat={chat} onClose={() => setConfirming(false)} />}
    </div>
  );
}
