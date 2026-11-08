import { useEffect, useState } from 'react';
import { fetchBeforeWorktree, setFetchBeforeWorktree } from '../../../ipc/worktrees';
import { useDeletePrefs } from '../../chats/deletePrefs.store';
import { CheckRow, Group } from '../SettingsRows';

/** Whether origin is fetched before Claude Code branches a worktree off it
 *  (Codex checks out the current commit instead). Kept by the Rust side,
 *  which does the fetch for chats agents start as well. */
function FetchRow() {
  const [on, setOn] = useState<boolean | null>(null);
  useEffect(() => {
    void fetchBeforeWorktree().then(setOn);
  }, []);
  return (
    <CheckRow
      label="Fetch origin before Claude Code makes a worktree"
      desc={
        <>
          Runs <code>git fetch</code> first: ten seconds at most, never asking for a password.
        </>
      }
      checked={on ?? true}
      disabled={on === null}
      onChange={v => void setFetchBeforeWorktree(v).then(setOn)}
    />
  );
}

/** Where the delete dialog's worktree checkbox starts. What deleting one
 *  loses stays in view. */
function DropWorktreeRow() {
  const on = useDeletePrefs(s => s.dropWorktree);
  return (
    <CheckRow
      label="Delete worktrees by default"
      desc="Sets the delete dialog’s initial worktree checkbox; you can still flip it per chat."
      note="Uncommitted work in a deleted worktree is lost."
      warn
      checked={on}
      onChange={v => useDeletePrefs.getState().setDropWorktree(v)}
    />
  );
}

export function WorktreesSection() {
  return (
    <Group
      title="Worktrees"
      about={{
        summary: 'How worktrees work',
        body: (
          <>
            <p>
              Claude Code branches its worktree off origin’s default branch as last fetched, and fetches it itself only
              once a day. With this on, Luna runs <code>git fetch</code> first — ten seconds at most, never asking for a
              password. Only origin’s branches move; nothing you have checked out is touched. Codex starts its worktree
              from the commit you have checked out, which no fetch changes.
            </p>
            <p>
              Where the worktree checkbox starts when you delete a chat that has one: Claude Code’s, in the project’s{' '}
              <code>.claude/worktrees</code>, goes with its <code>worktree-…</code> branch; Codex’s, in the account
              folder, has no branch of its own. It can still be flipped per chat; uncommitted work in a deleted worktree
              is lost.
            </p>
          </>
        )
      }}
    >
      <FetchRow />
      <DropWorktreeRow />
    </Group>
  );
}
