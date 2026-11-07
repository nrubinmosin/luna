import { useEffect, useState } from 'react';
import { codexHooksStart, setCodexHooksDefault } from '../../../ipc/trust';
import { fetchBeforeWorktree, setFetchBeforeWorktree } from '../../../ipc/worktrees';
import { useNewChat } from '../../new-chat/newchat.store';
import { Section } from '../../../shared/ui/Section';

/** One of the new-chat dialog's checkboxes, as it starts out. */
function StartRow({ label, which }: { label: string; which: 'worktree' | 'tools' }) {
  const on = useNewChat(s => s[which]);
  return (
    <div className="field-row" style={{ padding: '3px 0' }}>
      <input
        type="checkbox"
        id={`start-${which}`}
        checked={on}
        onChange={e => useNewChat.getState().setDefault(which, e.target.checked)}
      />
      <label htmlFor={`start-${which}`} style={{ cursor: 'default', fontSize: 'var(--fs-4)' }}>
        {label}
      </label>
    </div>
  );
}

/** Where the new-chat dialog's Codex hooks checkbox starts. Kept by the Rust
 *  side, which starts background helpers the same way (codex/hooks.rs). */
function HooksRow() {
  const [on, setOn] = useState<boolean | null>(null);
  useEffect(() => {
    void codexHooksStart('').then(s => setOn(s.default));
  }, []);
  return (
    <div className="field-row" style={{ padding: '3px 0' }}>
      <input
        type="checkbox"
        id="start-hooks"
        checked={on ?? false}
        disabled={on === null}
        onChange={e => void setCodexHooksDefault(e.target.checked).then(setOn)}
      />
      <label htmlFor="start-hooks" style={{ cursor: 'default', fontSize: 'var(--fs-4)' }}>
        Codex hooks without asking
      </label>
    </div>
  );
}

/** Whether origin is fetched before Claude Code branches a worktree off it
 *  (Codex checks out the current commit instead). Kept by the Rust side,
 *  which does the fetch for chats agents start as well. */
function FetchRow() {
  const [on, setOn] = useState<boolean | null>(null);
  useEffect(() => {
    void fetchBeforeWorktree().then(setOn);
  }, []);
  return (
    <div className="field-row" style={{ padding: '3px 0' }}>
      <input
        type="checkbox"
        id="fetch-toggle"
        checked={on ?? true}
        disabled={on === null}
        onChange={e => void setFetchBeforeWorktree(e.target.checked).then(setOn)}
      />
      <label htmlFor="fetch-toggle" style={{ cursor: 'default', fontSize: 'var(--fs-4)' }}>
        Fetch origin before Claude Code makes a worktree
      </label>
    </div>
  );
}

/** How a new chat starts: its checkboxes, and the fetch before a worktree. */
export function NewChatsSection() {
  return (
    <Section label="New chats">
      <StartRow label="Git worktree" which="worktree" />
      <StartRow label="Luna tools" which="tools" />
      <HooksRow />
      <div style={{ fontSize: 'var(--fs-1)', color: 'var(--faint)', marginTop: 4 }}>
        How the checkboxes start in a new chat, quick chats included. Flipping one is just for that chat; a folder that
        should start otherwise keeps its own — “keep for this folder” in the new-chat dialog. Codex runs a project’s
        hooks only once they are trusted, and asks again for every new worktree, account or edited hook; a helper in the
        background cannot ask and goes without them. Without asking, it runs them as they are. Helpers take what their
        folder would start with.
      </div>
      <FetchRow />
      <div style={{ fontSize: 'var(--fs-1)', color: 'var(--faint)', marginTop: 4 }}>
        Claude Code branches its worktree off origin’s default branch as last fetched, and fetches it itself only once a
        day. With this on, Luna runs <code>git fetch</code> first — ten seconds at most, never asking for a password.
        Only origin’s branches move; nothing you have checked out is touched. Codex starts its worktree from the commit
        you have checked out, which no fetch changes.
      </div>
    </Section>
  );
}
