import { useDeletePrefs } from '../../chats/deletePrefs.store';
import { Section } from '../../../shared/ui/Section';

/** Where the delete dialog's worktree checkbox starts. */
function DropWorktreeRow() {
  const on = useDeletePrefs(s => s.dropWorktree);
  return (
    <div className="field-row" style={{ padding: '3px 0' }}>
      <input
        type="checkbox"
        id="drop-worktree-toggle"
        checked={on}
        onChange={e => useDeletePrefs.getState().setDropWorktree(e.target.checked)}
      />
      <label htmlFor="drop-worktree-toggle" style={{ cursor: 'default', fontSize: 'var(--fs-4)' }}>
        Delete the worktree too
      </label>
    </div>
  );
}

export function DeletingSection() {
  return (
    <Section label="Deleting chats">
      <DropWorktreeRow />
      <div style={{ fontSize: 'var(--fs-1)', color: 'var(--faint)', marginTop: 4 }}>
        Where the worktree checkbox starts when you delete a chat that has one: Claude Code’s, in the project’s{' '}
        <code>.claude/worktrees</code>, goes with its <code>worktree-…</code> branch; Codex’s, in the account folder,
        has no branch of its own. It can still be flipped per chat; uncommitted work in a deleted worktree is lost.
      </div>
    </Section>
  );
}
