import { useState } from 'react';
import type { Chat } from '../../shared/types';
import { useChats } from './chats.store';
import { useAccounts } from '../accounts/accounts.store';
import { deleteSession } from '../../ipc/commands';
import { ConfirmDialog } from '../../shared/ui/ConfirmDialog';

/**
 * The one gate every "delete chat" click goes through, wherever the click
 * came from — the sidebar row or a pane's title bar. Owns the worktree
 * checkbox and the deletion itself, so the two entry points cannot drift.
 */
export function DeleteChatDialog({ chat, onClose }: { chat: Chat; onClose: () => void }) {
  const [dropWorktree, setDropWorktree] = useState(false);

  const doDelete = () => {
    onClose();
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
    useChats.getState().deleteChat(chat.id);
  };

  return (
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
      onCancel={onClose}
    />
  );
}
