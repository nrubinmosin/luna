import { useEffect, useState } from 'react';
import { helpersAboveCaller, setHelpersAboveCaller } from '../../../ipc/agents';
import { CheckRow, Group } from '../SettingsRows';

/** Whether a helper may run with more privilege than the session spawning it.
 *  Kept by the Rust side, which holds every spawn to it (agents/privilege.rs
 *  `cap`). What turning it on lets an agent do stays in view. */
export function HelpersSection() {
  const [on, setOn] = useState<boolean | null>(null);
  useEffect(() => {
    void helpersAboveCaller().then(setOn);
  }, []);
  return (
    <Group
      title="Helpers"
      about={{
        summary: 'How helper permissions work',
        body: (
          <p>
            Off, a chat’s helpers do no more without asking than it does: a Plan-only chat cannot spawn one that edits,
            an Ask chat none that bypasses permissions, and the same for Codex’s sandbox and network. A chat approves
            its helpers’ permission prompts only for what it runs unasked itself. On, any chat with Luna tools may spawn
            helpers that bypass permissions or run with full access, and approve what they ask, whatever it runs in
            itself. Chats you make yourself are not affected either way.
          </p>
        )
      }}
    >
      <CheckRow
        label="Allow helpers more permissions than their parent chat"
        desc="Off, a helper does no more without asking than the chat that spawned it."
        note="On, any chat with Luna tools may spawn helpers that bypass permissions or run with full access, and approve what they ask, whatever it runs in itself."
        warn
        checked={on ?? false}
        disabled={on === null}
        onChange={v => void setHelpersAboveCaller(v).then(setOn)}
      />
    </Group>
  );
}
