import { useState } from 'react';
import type { Chat } from '../../shared/types';
import { STATUS } from '../../shared/ui/status';
import { tint } from '../../shared/lib/format';
import { useChats } from './chats.store';
import { usePanes } from '../panes/panes.store';
import { useAccounts } from '../accounts/accounts.store';
import { deleteSession } from '../../ipc/commands';
import { ConfirmDialog } from '../../shared/ui/ConfirmDialog';

export function ChatRow({ chat }: { chat: Chat }) {
  const active = useChats(s => s.active === chat.id);
  const deleteChat = useChats(s => s.deleteChat);
  const setActive = useChats(s => s.setActive);
  const dragging = usePanes(s => s.drag === chat.id);
  const { setDrag, setOver, evictChat, setSpot } = usePanes.getState();
  const st = STATUS[chat.status];
  const [editing, setEditing] = useState(false);
  const [confirming, setConfirming] = useState(false);
  const [draft, setDraft] = useState('');

  const doDelete = () => {
    setConfirming(false);
    const folder = useChats.getState().folderOf(chat.id);
    const accountPath =
      useAccounts.getState().accounts.find(a => a.name === chat.account)?.path ?? '';
    if (folder) {
      // One command kills the process tree and clears worktree, branch and
      // attachments — it resolves the worktree itself, so deleting a chat
      // seconds after creating it no longer orphans one.
      void deleteSession(chat.id, folder.path, accountPath, chat.worktreePath ?? null).catch(err =>
        console.warn('[llm-desktop] delete failed', err)
      );
    }
    evictChat(chat.id);
    deleteChat(chat.id);
  };

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
      onMouseEnter={() => setSpot(chat.id)}
      onMouseLeave={() => setSpot(null)}
      title="Drag into a pane"
      className="hover-bg"
      style={{
        display: 'flex', alignItems: 'center', gap: 6, height: 25, padding: '0 6px',
        borderRadius: 2, cursor: 'grab',
        background: active ? tint(22, 'transparent') : 'transparent',
        opacity: dragging ? 0.45 : 1
      }}
    >
      <span
        title={st.label}
        style={{ width: 7, height: 7, borderRadius: '50%', flex: 'none', background: st.color, animation: st.anim }}
      />
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
          className="xp-field"
          style={{
            flex: '1 1 auto', minWidth: 40, height: 19, padding: '0 4px', border: '1px solid var(--input-border-color)',
            borderRadius: 1, background: '#fff', color: '#000', font: 'inherit', fontSize: 12.5, outline: 'none'
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
          style={{ flex: '1 1 auto', minWidth: 40, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap', fontSize: 12.5, fontWeight: 500 }}
        >
          {chat.name}
        </span>
      )}
      <span
        onClick={e => {
          e.stopPropagation();
          setConfirming(true);
        }}
        title={
          chat.worktree
            ? 'Delete chat — also removes its worktree and branch'
            : 'Delete chat'
        }
        className="hover-danger"
        style={{
          width: 16, height: 16, flex: 'none', borderRadius: 2, display: 'grid',
          placeItems: 'center', fontSize: 11, color: 'var(--faint)', cursor: 'default'
        }}
      >
        ✕
      </span>

      {confirming && (
        <ConfirmDialog
          title={`Delete "${chat.name}"?`}
          body={
            <>
              The session is stopped and its scrollback is lost.
              {chat.worktree && (
                <>
                  {' '}Its git worktree{chat.worktreePath ? ` (${chat.worktreePath})` : ''} and the
                  throwaway <code>worktree-…</code> branch go with it, including any uncommitted work.
                </>
              )}{' '}
              Pasted attachments are removed. The transcript is kept, so the session can still be
              resumed from the CLI.
            </>
          }
          onConfirm={doDelete}
          onCancel={() => setConfirming(false)}
        />
      )}
    </div>
  );
}
