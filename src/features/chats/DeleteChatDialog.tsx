import { type ReactNode, useEffect, useMemo, useState } from 'react';
import type { Chat } from '../../shared/types';
import { useChats } from './chats.store';
import { ui } from '../providers';
import { backgroundAtStake, backgroundDelete, type AtStakeDto } from '../../ipc/agents';
import { deleteSession } from '../../ipc/worktrees';
import { ConfirmDialog } from '../../shared/ui/ConfirmDialog';
import { rootOf, useBackground } from '../agents/background.store';
import { useDeletePrefs } from './deletePrefs.store';
import { helperDeletes, helperLines } from './helperDeletes';
import { logWarn } from '../../shared/lib/log';

/**
 * The one gate every "delete chat" click goes through, wherever the click
 * came from — the sidebar row or a pane's title bar. Owns the worktree
 * checkbox, the helpers its agent runs, and the deletion itself, so the two
 * entry points cannot drift.
 */
function deleteOne(chat: Chat, dropWorktree: boolean) {
  const folder = useChats.getState().folderOf(chat.id);
  if (folder) {
    // One command kills the process tree and clears the attachments — and the
    // worktree and its branch only if that was asked for. It resolves the
    // worktree itself, so deleting a chat seconds after creating it no longer
    // orphans one.
    void deleteSession(chat.id, folder.path, chat.worktreePath ?? null, dropWorktree).catch(err =>
      console.warn('[luna] delete failed', err)
    );
  }
  // Both stores clear the panes themselves, open or not.
  useChats.getState().deleteChat(chat.id);
}

/** A box with its label, drawn by xp.css on the label: the input needs an id
 *  and the label a matching `for`, or the box renders as nothing at all. */
function Box({
  id,
  checked,
  onChange,
  children
}: {
  id: string;
  checked: boolean;
  onChange: (on: boolean) => void;
  children: ReactNode;
}) {
  return (
    <div style={{ display: 'flex', alignItems: 'flex-start', gap: 7, fontSize: 'var(--fs-4)', color: 'var(--dim)' }}>
      <input type="checkbox" id={id} checked={checked} onChange={e => onChange(e.target.checked)} />
      {/* xp.css makes every label inline-flex, which chops a wrapping label
          into unwrappable flex columns around a <code> child; block restores
          normal text flow. */}
      <label htmlFor={id} style={{ display: 'block', cursor: 'default', lineHeight: 1.5 }}>
        {children}
      </label>
    </div>
  );
}

const list = (names: string[]) => names.join(', ');

/** Two spellings of one folder: either slash, a trailing one, case aside. */
const samePath = (a: string, b: string) => {
  const key = (p: string) =>
    p
      .replace(/[\\/]+/g, '/')
      .replace(/\/$/, '')
      .toLowerCase();
  return key(a) === key(b);
};

export function DeleteChatDialog({ chat, onClose }: { chat: Chat; onClose: () => void }) {
  // Starts where Settings says; flipping it here is for this delete only.
  const [dropWorktree, setDropWorktree] = useState(() => useDeletePrefs.getState().dropWorktree);
  // The helpers this chat's agent runs. Kept by default — they may hold an
  // answer nothing else has — and each goes by itself once idle.
  const [withHelpers, setWithHelpers] = useState(false);
  // A yes of its own each: what the chat's boxes never stretch to.
  const [stopWorking, setStopWorking] = useState(false);
  const [dropUnsaved, setDropUnsaved] = useState(false);
  const allBackground = useBackground(s => s.helpers);
  const helpers = useMemo(
    () => allBackground.filter(h => rootOf(h, allBackground) === chat.id),
    [allBackground, chat.id]
  );
  const ids = helpers.map(h => h.id).join('\n');
  // What each has at stake, as the core sees it now: its turn, its worktree's
  // unsaved work. Asked once the list is known; the rows alone until it answers.
  const [stake, setStake] = useState<Record<string, AtStakeDto> | null>(null);
  useEffect(() => {
    if (!ids) return;
    let live = true;
    backgroundAtStake(ids.split('\n'))
      .then(rows => {
        if (live) setStake(Object.fromEntries(rows.map(r => [r.id, r])));
      })
      .catch(e => logWarn('agents', `asking what the helpers have at stake failed: ${String(e)}`));
    return () => {
      live = false;
    };
  }, [ids]);
  // Another chat in the same worktree (one moved to another account goes on
  // in its old chat's): never dropped under it, whatever the box says.
  const sharedWith = useChats(s =>
    chat.worktreePath
      ? s.folders
          .flatMap(f => f.chats)
          .find(c => c.id !== chat.id && c.worktreePath && samePath(c.worktreePath, chat.worktreePath ?? ''))
      : undefined
  );
  const lines = helperLines(helpers, stake);
  const working = lines.filter(l => l.working);
  const unsaved = lines.filter(l => l.unsaved);

  const doDelete = () => {
    onClose();
    for (const d of helperDeletes(lines, { withHelpers, dropWorktree, stopWorking, dropUnsaved })) {
      void backgroundDelete(d.id, d)
        .then(r => {
          if (r.worktreeKept)
            logWarn('agents', `helper ${d.id} deleted, its worktree kept for its unsaved work: ${r.worktreeKept}`);
        })
        .catch(err => console.warn('[luna] delete failed', err));
    }
    deleteOne(chat, dropWorktree && !sharedWith);
  };

  return (
    <ConfirmDialog
      // Trimmed here rather than left to the ellipsis in the title bar, so
      // the closing quote and question mark survive a long chat name.
      title={`Delete "${chat.name.length > 38 ? `${chat.name.slice(0, 38)}…` : chat.name}"?`}
      body={
        <>
          The session is stopped and its scrollback is lost. Pasted attachments are removed. The transcript is kept, so
          the session can still be resumed from the CLI.
          {chat.worktree && sharedWith ? (
            <> Its git worktree stays: "{sharedWith.name}" works in it too.</>
          ) : (
            chat.worktree &&
            (dropWorktree ? (
              <> Its git worktree goes too, unless you untick the box below.</>
            ) : (
              <> Its git worktree stays on disk unless you say otherwise.</>
            ))
          )}
          {chat.worktree && chat.worktreePath && (
            // On its own line: a Windows worktree path is long enough to
            // break the sentence it sits in across three ragged lines.
            <div
              title={chat.worktreePath}
              style={{
                marginTop: 8,
                padding: '4px 6px',
                borderRadius: 2,
                background: 'var(--chip)',
                fontFamily: 'ui-monospace, Consolas, monospace',
                fontSize: 'var(--fs-3)',
                color: 'var(--faint)',
                overflow: 'hidden',
                textOverflow: 'ellipsis',
                whiteSpace: 'nowrap'
              }}
            >
              {chat.worktreePath}
            </div>
          )}
          {lines.length > 0 && (
            <ul className="helper-stakes" style={{ margin: '8px 0 0', paddingLeft: 18, fontSize: 'var(--fs-3)' }}>
              {lines.map(l => (
                <li key={l.id}>
                  <b>{l.name}</b>: {l.working ?? 'done'}
                  {l.unsaved && <> · its worktree holds {l.unsaved}</>}
                </li>
              ))}
            </ul>
          )}
        </>
      }
      extra={
        (chat.worktree || lines.length > 0) && (
          <div onClick={e => e.stopPropagation()} style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
            {((chat.worktree && !sharedWith) || (withHelpers && helpers.some(h => h.worktree))) && (
              <Box id={`drop-worktree-${chat.id}`} checked={dropWorktree} onChange={setDropWorktree}>
                {ui(chat.provider).branchPrefix ? (
                  <>
                    Delete the worktree and its throwaway <code>{ui(chat.provider).branchPrefix}…</code> branch too,
                  </>
                ) : (
                  <>Delete the worktree too,</>
                )}{' '}
                including any uncommitted work.
              </Box>
            )}
            {lines.length > 0 && (
              <Box id={`drop-helpers-${chat.id}`} checked={withHelpers} onChange={setWithHelpers}>
                Also delete the {lines.length} helper{lines.length > 1 ? 's' : ''} its agent runs. Left alone, each goes
                by itself half an hour after it is done.
              </Box>
            )}
            {withHelpers && working.length > 0 && (
              <Box id={`stop-working-${chat.id}`} checked={stopWorking} onChange={setStopWorking}>
                Also stop and delete the {working.length === 1 ? 'one' : working.length} still working (
                {list(working.map(l => l.name))}). Left unticked, {working.length === 1 ? 'it finishes' : 'they finish'}{' '}
                first.
              </Box>
            )}
            {withHelpers && dropWorktree && unsaved.length > 0 && (
              <Box id={`drop-unsaved-${chat.id}`} checked={dropUnsaved} onChange={setDropUnsaved}>
                Also delete the worktree{unsaved.length > 1 ? 's' : ''} of {list(unsaved.map(l => l.name))}, with the
                uncommitted work in {unsaved.length > 1 ? 'them' : 'it'}. Left unticked,{' '}
                {unsaved.length > 1 ? 'they stay' : 'it stays'} on disk.
              </Box>
            )}
          </div>
        )
      }
      onConfirm={doDelete}
      onCancel={onClose}
    />
  );
}
