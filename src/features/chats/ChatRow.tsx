import { useState } from 'react';
import type { Chat } from '../../shared/types';
import { StatusDot } from '../../shared/ui/StatusDot';
import { chatColorTheme } from '../../shared/ui/chatColors';
import { ACCENT, tint } from '../../shared/lib/format';
import { numberedChats, useChats } from './chats.store';
import { currentLayout, currentSlots, usePanes } from '../panes/panes.store';
import { DeleteChatDialog } from './DeleteChatDialog';

export function ChatRow({ chat }: { chat: Chat }) {
  const active = useChats(s => s.active === chat.id);
  const setActive = useChats(s => s.setActive);
  const dragging = usePanes(s => s.drag === chat.id);
  const layout = usePanes(currentLayout);
  const group = usePanes(s => s.group);
  // Which tile of the board on screen holds this chat, 1-based to match the
  // "pane N of M" wording in an empty pane; null when it isn't on this board.
  const paneNo = usePanes(s => {
    const i = currentSlots(s).indexOf(chat.id);
    return i >= 0 && i < currentLayout(s) ? i + 1 : null;
  });
  // With one pane there are no pane numbers worth showing, so the row wears its
  // own place in the list instead — which is what Ctrl+<digit> counts through.
  const listNo = useChats(s => {
    if (layout > 1) return null;
    const i = numberedChats(s.folders, group).findIndex(c => c.id === chat.id);
    return i >= 0 && i < 9 ? i + 1 : null;
  });
  const badge = layout === 1 ? listNo : paneNo;
  const onScreen = paneNo != null;

  const { setDrag, setOver, setSpot, showChat } = usePanes.getState();
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
      // One click shows it. With a single pane that means the pane swaps to
      // this chat — dragging a row across an empty board to the one place it
      // can go was the ceremony this replaces. With more panes the board is
      // an arrangement, so a chat that isn't on it comes up on the peek sheet
      // instead of pushing something out. See showChat.
      onClick={() => {
        setActive(chat.id);
        showChat(chat.id);
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
          by eye. In flow whether set or not. */}
      <span
        title={chat.color ? 'Chat colour — set from the pane title bar' : undefined}
        style={{
          width: 3, height: 15, borderRadius: 2, flex: 'none',
          background: chatColorTheme(chat.color)?.swatch ?? 'transparent'
        }}
      />
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
      {badge != null && (
        <span
          title={
            layout === 1
              ? `Ctrl+${badge} shows this chat`
              : `Showing in pane ${badge} of the current layout — Ctrl+${badge} holds it up on its own`
          }
          style={{
            minWidth: 15, height: 15, padding: '0 2px', flex: 'none', borderRadius: 2,
            display: 'grid', placeItems: 'center', fontSize: 'var(--fs-1)', fontWeight: 600,
            fontVariantNumeric: 'tabular-nums',
            color: onScreen ? '#fff' : 'var(--faint)',
            background: onScreen ? ACCENT : 'transparent',
            border: `1px solid ${onScreen ? ACCENT : 'var(--line)'}`
          }}
        >
          {badge}
        </span>
      )}
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
