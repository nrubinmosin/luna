import { useEffect, useState } from 'react';
import { helpersAboveCaller, setHelpersAboveCaller } from '../../../ipc/agents';
import { Section } from '../../../shared/ui/Section';

/** Whether a helper may run with more privilege than the session spawning it.
 *  Kept by the Rust side, which holds every spawn to it (agents/privilege.rs `cap`). */
function HelpersAboveRow() {
  const [on, setOn] = useState<boolean | null>(null);
  useEffect(() => {
    void helpersAboveCaller().then(setOn);
  }, []);
  return (
    <div className="field-row" style={{ padding: '3px 0' }}>
      <input
        type="checkbox"
        id="helpers-above"
        checked={on ?? false}
        disabled={on === null}
        onChange={e => void setHelpersAboveCaller(e.target.checked).then(setOn)}
      />
      <label htmlFor="helpers-above" style={{ cursor: 'default', fontSize: 'var(--fs-4)' }}>
        Let helpers run with more privilege than the chat that spawns them
      </label>
    </div>
  );
}

export function HelpersSection() {
  return (
    <Section label="Helpers">
      <HelpersAboveRow />
      <div style={{ fontSize: 'var(--fs-1)', color: 'var(--faint)', marginTop: 4 }}>
        Off, a chat’s helpers do no more without asking than it does: a Plan-only chat cannot spawn one that edits, an
        Ask chat none that bypasses permissions, and the same for Codex’s sandbox and network. A chat approves its
        helpers’ permission prompts only for what it runs unasked itself. On, any chat with Luna tools may spawn helpers
        that bypass permissions or run with full access, and approve what they ask, whatever it runs in itself. Chats
        you make yourself are not affected either way.
      </div>
    </Section>
  );
}
