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
  const { setDrag, setOver, setSpot } = usePanes.getState();
  const st = STATUS[chat.status];
  const [editing, setEditing] = useState(false);
  const [confirming, setConfirming] = useState(false);
  const [dropWorktree, setDropWorktree] = useState(false);
  const [draft, setDraft] = useState('');

  const doDelete = () => {
    setConfirming(false);
    const folder = useChats.getState().folderOf(chat.id);
    const accountPath =
      useAccounts.getState().accounts.find(a => a.name === chat.account)?.path ?? '';
    if (folder) {
      // One command kills the process tree and clears the attachments — and the
      // worktree and its branch only if that was asked for. It resolves the
      // worktree itself, so deleting a chat seconds after creating it no longer
      // orphans one.
      void deleteSession(
        chat.id,
        folder.path,
        accountPath,
        chat.worktreePath ?? null,
        dropWorktree
      ).catch(err => console.warn('[luna] delete failed', err));
    }
    // Both stores clear the panes themselves, open or not.
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
      className="hover-bg chat-row"
      style={{
        display: 'flex', alignItems: 'center', gap: 6, height: 27, padding: '0 6px',
        borderRadius: 2, cursor: 'grab',
        background: active ? tint(22, 'transparent') : 'transparent',
        opacity: dragging ? 0.45 : 1
      }}
    >
      {/* Kept in flow whether it is set or not — fading it in on hover instead
          of adding it would shift every name in the list on mouse-over. */}
      <span
        onClick={e => {
          e.stopPropagation();
          useChats.getState().toggleMark(chat.id);
        }}
        title={chat.marked ? 'Marked — click to clear' : 'Mark this chat'}
        className="mark"
        data-on={chat.marked ? 'yes' : 'no'}
      >
        {chat.marked ? '★' : '☆'}
      </span>
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
          title="Double-click to rename"
          style={{ flex: '1 1 auto', minWidth: 40, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap', fontSize: 'var(--fs-4)', fontWeight: 500 }}
        >
          {chat.name}
        </span>
      )}
      <span
        onClick={e => {
          e.stopPropagation();
          useChats.getState().setArchived(chat.id, !chat.archived);
        }}
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
          setDropWorktree(false);
          setConfirming(true);
        }}
        title="Delete chat"
        className="hover-danger"
        style={{
          width: 17, height: 17, flex: 'none', borderRadius: 2, display: 'grid',
          placeItems: 'center', fontSize: 'var(--fs-2)', color: 'var(--faint)', cursor: 'default'
        }}
      >
        ✕
      </span>

      {confirming && (
        <ConfirmDialog
          // Trimmed here rather than left to the ellipsis in the title bar, so
          // the closing quote and question mark survive a long chat name.
          title={`Delete "${chat.name.length > 38 ? `${chat.name.slice(0, 38)}…` : chat.name}"?`}
          body={
            <>
              The session is stopped and its scrollback is lost. Pasted attachments are removed.
              The transcript is kept, so the session can still be resumed from the CLI.
              {chat.worktree && <> Its git worktree stays on disk unless you say otherwise.</>}
              {chat.worktree && chat.worktreePath && (
                // On its own line: a Windows worktree path is long enough to
                // break the sentence it sits in across three ragged lines.
                <div
                  title={chat.worktreePath}
                  style={{
                    marginTop: 8, padding: '4px 6px', borderRadius: 2, background: 'var(--chip)',
                    fontFamily: 'ui-monospace, Consolas, monospace', fontSize: 'var(--fs-3)',
                    color: 'var(--faint)', overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap'
                  }}
                >
                  {chat.worktreePath}
                </div>
              )}
            </>
          }
          extra={
            chat.worktree && (
              // The box itself is drawn by xp.css on the label, which is why
              // the input needs an id and the label a matching `for`: nested
              // inside one, the checkbox renders as nothing at all.
              <div
                onClick={e => e.stopPropagation()}
                style={{ display: 'flex', alignItems: 'flex-start', gap: 7, fontSize: 'var(--fs-4)', color: 'var(--dim)' }}
              >
                <input
                  type="checkbox"
                  id={`drop-worktree-${chat.id}`}
                  checked={dropWorktree}
                  onChange={e => setDropWorktree(e.target.checked)}
                />
                {/* xp.css makes every label inline-flex, which chops a wrapping
                    label into unwrappable flex columns around the <code> child;
                    block restores normal text flow. */}
                <label htmlFor={`drop-worktree-${chat.id}`} style={{ display: 'block', cursor: 'default', lineHeight: 1.5 }}>
                  Delete the worktree and its throwaway <code>worktree-…</code> branch too,
                  including any uncommitted work.
                </label>
              </div>
            )
          }
          onConfirm={doDelete}
          onCancel={() => setConfirming(false)}
        />
      )}
    </div>
  );
}
