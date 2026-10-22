import { useState } from 'react';
import type { Chat } from '../../shared/types';
import { StatusDot } from '../../shared/ui/StatusDot';
import { chatColorTheme } from '../../shared/ui/chatColors';
import { ACCENT, shortSessionId } from '../../shared/lib/format';
import { Icon, ToolButton } from '../../shared/ui/icons';
import { numberedChats, useChats } from './chats.store';
import { currentLayout, currentSlots, usePanes } from '../panes/panes.store';
import { DeleteChatDialog } from './DeleteChatDialog';
import { BackgroundChip } from '../agents/BackgroundChip';

export function ChatRow({
  chat,
  depth = 0,
  childCount = 0,
  orphan = false
}: {
  chat: Chat;
  /** Nesting under the chat whose agent spawned it (FolderSection.tree). */
  depth?: number;
  /** Chats this one's agent spawned, shown when unfolded. */
  childCount?: number;
  /** Spawned, yet listed on its own: the parent is gone, or sits in
   *  another folder. */
  orphan?: boolean;
}) {
  const active = useChats(s => s.active === chat.id);
  // An agent may spawn into any folder, so a parent missing from this one
  // is not necessarily gone.
  const parentName = useChats(s => (orphan ? s.findChat(chat.parentId ?? null)?.name ?? null : null));
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
      // One click shows it full size. With a single pane that means the pane
      // swaps to this chat — dragging a row across an empty board to the one
      // place it can go was the ceremony this replaces. With more panes the
      // board is an arrangement, so the chat comes up on the peek sheet
      // instead of pushing something out, and a second click on the same row
      // puts it back. See showChat. With Alt down the row is already up from
      // the hover (useAltPeek), and the click only keeps it there.
      onClick={e => {
        setActive(chat.id);
        if (!e.altKey) showChat(chat.id);
      }}
      onMouseEnter={() => setSpot(chat.id)}
      onMouseLeave={() => setSpot(null)}
      className="chat-row"
      data-active={active ? 'yes' : 'no'}
      data-waiting={chat.status === 'waiting' ? 'yes' : 'no'}
      style={{ paddingLeft: 5 + depth * 14, opacity: dragging ? 0.45 : 1 }}
    >
      {/* The chat's colour, so a row and the pane wearing the same tint link up
          by eye. In flow whether set or not. */}
      <span
        title={chat.color ? 'Chat colour — set from the pane title bar' : undefined}
        style={{
          width: 4, height: 17, borderRadius: 2, flex: 'none',
          background: chatColorTheme(chat.color)?.swatch ?? 'transparent'
        }}
      />
      {/* Sessions this chat's agent spawned, folded by default. The count
          stays visible folded, so a parent at work is never a mystery. */}
      {childCount > 0 && (
        <span
          onClick={e => {
            e.stopPropagation();
            useChats.getState().setChildrenOpen(chat.id, !chat.childrenOpen);
          }}
          title={`${childCount} session${childCount > 1 ? 's' : ''} spawned by this chat — click to ${chat.childrenOpen ? 'fold' : 'unfold'}`}
          className="kids"
          data-open={chat.childrenOpen ? 'yes' : 'no'}
        >
          <svg width="9" height="9" viewBox="0 0 9 9" aria-hidden>
            <path d="M3 1.5 6 4.5 3 7.5" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round" />
          </svg>
          {childCount}
        </span>
      )}
      {/* Helpers its agent runs in the background: no rows, a count here. */}
      <BackgroundChip parentId={chat.id} />
      {(depth > 0 || orphan) && (
        <span
          title={
            !orphan
              ? 'Spawned by the chat above'
              : parentName
                ? `Spawned by "${parentName}" in another folder`
                : 'Spawned by a chat that no longer exists'
          }
          style={{ flex: 'none', color: 'var(--faint)', display: 'flex' }}
        >
          <Icon name="child" size={13} />
        </span>
      )}
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
          // The full name first: the row is narrow enough that the ellipsis is
          // the common case. The session id is how issue claims and logs name
          // this chat.
          title={[chat.name, chat.sessionId && `Session ${shortSessionId(chat.sessionId)}  (${chat.sessionId})`]
            .filter(Boolean)
            .join('\n')}
          style={{
            flex: '1 1 auto', minWidth: 40, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap',
            fontSize: 'var(--fs-4)', fontWeight: chat.status === 'waiting' ? 700 : 500
          }}
        >
          {chat.name}
        </span>
      )}
      {chat.status === 'waiting' && !editing && <span className="row-tag">waiting</span>}
      {/* Sits after the flexible name, so appearing or vanishing on a layout
          switch only nudges the controls on the right, never the names. */}
      {badge != null && (
        <span
          title={layout === 1 ? `Number ${badge} in the list` : `Showing in pane ${badge}`}
          style={{
            minWidth: 18, height: 18, padding: '0 3px', flex: 'none', borderRadius: 4,
            display: 'grid', placeItems: 'center', fontSize: 'var(--fs-1)', fontWeight: 600,
            fontVariantNumeric: 'tabular-nums',
            color: onScreen ? '#fff' : 'inherit',
            opacity: onScreen ? 1 : 0.6,
            background: onScreen ? ACCENT : 'transparent',
            border: `1px solid ${onScreen ? (active ? '#fff' : ACCENT) : 'currentColor'}`,
            transition: 'background .2s, color .2s'
          }}
        >
          {badge}
        </span>
      )}
      <span className="row-tools">
        <ToolButton
          icon="pencil"
          size={22}
          title="Rename chat"
          onClick={() => {
            setDraft(chat.name);
            setEditing(true);
          }}
        />
        <ToolButton icon="trash" size={22} danger title="Delete chat — asks first" onClick={() => setConfirming(true)} />
      </span>

      {confirming && <DeleteChatDialog chat={chat} onClose={() => setConfirming(false)} />}
    </div>
  );
}
