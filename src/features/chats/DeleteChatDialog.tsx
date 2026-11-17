import { type ReactNode, useEffect, useMemo, useState } from 'react';
import type { Chat } from '../../shared/types';
import { useChats } from './chats.store';
import { ui } from '../providers';
import {
  backgroundAtStake,
  backgroundDelete,
  backgroundMove,
  backgroundMoveTargets,
  type AtStakeDto,
  type MoveTargetsDto
} from '../../ipc/agents';
import { deleteSession } from '../../ipc/worktrees';
import { ConfirmDialog } from '../../shared/ui/ConfirmDialog';
import { helpersUnder, useBackground } from '../agents/background.store';
import { useNameOf } from '../agents/useNameOf';
import { useDeletePrefs } from './deletePrefs.store';
import { helperDeletes, helperLines, helperMoves, movingWith, preselectPass } from './helperDeletes';
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
  disabled = false,
  children
}: {
  id: string;
  checked: boolean;
  onChange: (on: boolean) => void;
  disabled?: boolean;
  children: ReactNode;
}) {
  return (
    <div style={{ display: 'flex', alignItems: 'flex-start', gap: 7, fontSize: 'var(--fs-4)', color: 'var(--dim)' }}>
      <input type="checkbox" id={id} checked={checked} disabled={disabled} onChange={e => onChange(e.target.checked)} />
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

/** A chat's name, or why not, short enough for a picker's line. */
const clip = (s: string, n: number) => (s.length > n ? `${s.slice(0, n - 1)}…` : s);

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
  const helpers = useMemo(() => helpersUnder(allBackground, chat.id), [allBackground, chat.id]);
  const ids = helpers.map(h => h.id).join('\n');
  // Only the chat's own helpers pass to another chat; the helpers under one go with it.
  const own = helpers
    .filter(h => h.parent === chat.id)
    .map(h => h.id)
    .join('\n');
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
  // The person's pick per helper ('' for none). Each starts, once its targets
  // are in, on a chat that asked for it or waits on it then, and stays so: a
  // wait ending or a chat asking later never flips a helper between passed
  // and left behind the person's back. Moved only on Delete.
  const [picked, setPicked] = useState<Record<string, string>>({});
  // Where each of its own helpers may go, asked once the list is known.
  const [targets, setTargets] = useState<Record<string, MoveTargetsDto>>({});
  useEffect(() => {
    if (!own) return;
    let live = true;
    void Promise.all(
      own.split('\n').map(id =>
        backgroundMoveTargets(id).then(
          (t): [string, MoveTargetsDto] => [id, t],
          (e): [string, MoveTargetsDto] => {
            logWarn('agents', `listing where a helper may move failed: ${String(e)}`);
            return [id, { blocked: String(e), targets: [] }];
          }
        )
      )
    ).then(rows => {
      if (!live) return;
      setTargets(Object.fromEntries(rows));
      const now = useBackground.getState().helpers;
      setPicked(p => {
        const next = { ...p };
        for (const [id, t] of rows)
          next[id] ??=
            preselectPass(
              now.find(h => h.id === id),
              t
            ) ?? '';
        return next;
      });
    });
    return () => {
      live = false;
    };
  }, [own]);
  // Delete waits for where its own helpers may go: until then neither the
  // pickers nor what Delete would do with each is shown.
  const looking = own.split('\n').some(id => id && !targets[id]);
  const [busy, setBusy] = useState(false);
  const [refused, setRefused] = useState<string | null>(null);
  const nameOf = useNameOf();
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
  const passing: Record<string, string> = {};
  for (const h of helpers) {
    const to = picked[h.id];
    if (to) passing[h.id] = to;
  }
  const moving = movingWith(lines, passing);
  // What stays to be deleted or left: the helpers not passed on.
  const left = lines.filter(l => !moving.has(l.id));
  const working = left.filter(l => l.working);
  const unsaved = left.filter(l => l.unsaved);

  const doDelete = async () => {
    if (busy || looking) return;
    // What the person saw and ticked; the controls stay frozen while the moves run.
    const shown = new Set(lines.map(l => l.id));
    const choices = { withHelpers, dropWorktree, stopWorking, dropUnsaved };
    // The moves first, one by one: a refusal stops here, with nothing deleted.
    const moves = helperMoves(lines, passing);
    if (moves.length > 0) {
      setBusy(true);
      setRefused(null);
      const done: string[] = [];
      for (const m of moves) {
        try {
          await backgroundMove(m.id, m.to);
          done.push(m.name);
        } catch (e) {
          logWarn('agents', `passing a helper on before a delete failed: ${String(e)}`);
          setBusy(false);
          setRefused(
            `Nothing was deleted: "${m.name}" did not move to "${nameOf(m.to)}": ${String(e)}` +
              (done.length ? ` (${list(done)} moved already.)` : '')
          );
          return;
        }
      }
    }
    onClose();
    // What is left to delete, looked at again after the moves: a helper another
    // window moved elsewhere meanwhile is this chat's no more. The rows may not
    // show a move just made yet, so what was passed stays out by its own list,
    // and the core refuses one that hangs from another chat by now (`ofChat`).
    const now = useBackground.getState().helpers;
    const still = helperLines(helpersUnder(now, chat.id), stake);
    const passed = new Set([...moving, ...movingWith(still, passing)]);
    for (const d of helperDeletes(still, choices, passed, shown)) {
      void backgroundDelete(d.id, { ...d, ofChat: chat.id })
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
              {lines.map(l => {
                const t = targets[l.id];
                // The chat going is no chat to pass it to.
                const to = (t?.blocked ? [] : (t?.targets ?? [])).filter(x => x.id !== chat.id);
                const not = to.filter(x => !x.ok);
                const goesWith = !passing[l.id] && moving.has(l.id) && l.parent ? nameOf(l.parent) : null;
                return (
                  <li key={l.id}>
                    <b>{l.name}</b>: {l.working ?? 'done'}
                    {l.unsaved && <> · its worktree holds {l.unsaved}</>}
                    {goesWith && <> · goes with {goesWith}</>}
                    {t?.blocked && (
                      <div className="pass-blocked" style={{ color: 'var(--wait)' }}>
                        Cannot be passed to a chat now: {t.blocked}
                      </div>
                    )}
                    {to.length > 0 && (
                      <div>
                        <select
                          className="pass-to"
                          aria-label={`Pass "${l.name}" to a chat`}
                          value={passing[l.id] ?? ''}
                          disabled={busy}
                          onChange={e => setPicked(p => ({ ...p, [l.id]: e.target.value }))}
                          style={{ maxWidth: '100%', marginTop: 2 }}
                        >
                          <option value="">Pass to chat…</option>
                          {to.map(x => (
                            <option key={x.id} value={x.id} disabled={!x.ok}>
                              {x.ok ? `Pass to ${clip(nameOf(x.id), 48)}` : `${clip(nameOf(x.id), 40)} (not possible)`}
                            </option>
                          ))}
                        </select>
                        {/* An option's line is cut, and Chromium shows no tooltip on
                            one: each why in full, folded away until asked for. */}
                        {not.length > 0 && (
                          <details className="pass-why">
                            <summary>
                              Why not{' '}
                              {not.length === 1 && not[0]
                                ? `"${clip(nameOf(not[0].id), 40)}"`
                                : `the other ${not.length} chats`}
                            </summary>
                            {not.map(x => (
                              <div key={x.id}>
                                {nameOf(x.id)}: {x.why ?? 'not possible'}
                              </div>
                            ))}
                          </details>
                        )}
                      </div>
                    )}
                  </li>
                );
              })}
            </ul>
          )}
          {refused && (
            <div role="alert" style={{ marginTop: 8, color: 'var(--wait)' }}>
              {refused}
            </div>
          )}
        </>
      }
      extra={
        (chat.worktree || left.length > 0) && (
          <div onClick={e => e.stopPropagation()} style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
            {((chat.worktree && !sharedWith) ||
              (withHelpers && helpers.some(h => h.worktree && !moving.has(h.id)))) && (
              <Box id={`drop-worktree-${chat.id}`} checked={dropWorktree} onChange={setDropWorktree} disabled={busy}>
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
            {left.length > 0 && (
              <Box id={`drop-helpers-${chat.id}`} checked={withHelpers} onChange={setWithHelpers} disabled={busy}>
                Also delete the {left.length} helper{left.length > 1 ? 's' : ''} its agent runs
                {moving.size > 0 && ' that you do not pass to a chat'}. Left alone, the chat's own helpers answer to no
                chat from then on, and each deletes itself half an hour after it is done and no helper of its own is
                left; the helpers under one stay under it.
              </Box>
            )}
            {withHelpers && working.length > 0 && (
              <Box id={`stop-working-${chat.id}`} checked={stopWorking} onChange={setStopWorking} disabled={busy}>
                Also stop and delete the {working.length === 1 ? 'one' : working.length} still working (
                {list(working.map(l => l.name))}). Left unticked, {working.length === 1 ? 'it finishes' : 'they finish'}{' '}
                first.
              </Box>
            )}
            {withHelpers && dropWorktree && unsaved.length > 0 && (
              <Box id={`drop-unsaved-${chat.id}`} checked={dropUnsaved} onChange={setDropUnsaved} disabled={busy}>
                Also delete the worktree{unsaved.length > 1 ? 's' : ''} of {list(unsaved.map(l => l.name))}, with the
                uncommitted work in {unsaved.length > 1 ? 'them' : 'it'}. Left unticked,{' '}
                {unsaved.length > 1 ? 'they stay' : 'it stays'} on disk.
              </Box>
            )}
          </div>
        )
      }
      confirmLabel={busy ? 'Moving…' : 'Delete'}
      confirmDisabled={busy || looking}
      onConfirm={() => void doDelete()}
      // Not while the moves run: the delete they lead to would go on unseen.
      onCancel={busy ? () => {} : onClose}
    />
  );
}
