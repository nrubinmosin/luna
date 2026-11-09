import { useEffect, useState } from 'react';
import { codexHooksStart, setCodexHooksDefault } from '../../../ipc/trust';
import { useNewChat, type Start } from '../../new-chat/newchat.store';
import { CheckRow, Group } from '../SettingsRows';

/** One of the new-chat dialog's checkboxes, as it starts out. */
function StartRow({ label, desc, which }: { label: string; desc: string; which: keyof Start }) {
  const on = useNewChat(s => s[which]);
  return <CheckRow label={label} desc={desc} checked={on} onChange={v => useNewChat.getState().setDefault(which, v)} />;
}

/** Where the new-chat dialog's Codex hooks checkbox starts. Kept by the Rust
 *  side, which starts background helpers the same way (codex/hooks.rs). */
function HooksRow() {
  const [on, setOn] = useState<boolean | null>(null);
  useEffect(() => {
    void codexHooksStart('').then(s => setOn(s.default));
  }, []);
  return (
    <CheckRow
      label="Codex hooks without asking"
      desc="Codex runs a project’s hooks without asking you to trust them first."
      checked={on ?? false}
      disabled={on === null}
      onChange={v => void setCodexHooksDefault(v).then(setOn)}
    />
  );
}

/** How a new chat starts: the new-chat dialog's checkboxes. */
export function NewChatsSection() {
  return (
    <Group
      title="New chats start with"
      about={{
        summary: 'How new-chat defaults work',
        body: (
          <p>
            How the checkboxes start in a new chat, quick chats included. Flipping one is just for that chat; a folder
            that should start otherwise keeps its own — “keep for this folder” in the new-chat dialog. Codex runs a
            project’s hooks only once they are trusted, and asks again for every new worktree, account or edited hook; a
            helper in the background cannot ask and goes without them. Without asking, it runs them as they are. Helpers
            take the hooks their folder would start with. The wait guard refuses a shell command that would wait
            forever, such as following a log with no deadline; Codex runs it only with its hooks without asking. Helpers
            have it whatever this says.
          </p>
        )
      }}
    >
      <StartRow label="Git worktree" desc="A new chat starts in a worktree of its own." which="worktree" />
      <StartRow label="Luna tools" desc="A new chat gets Luna’s tools, such as starting helpers." which="tools" />
      <HooksRow />
      <StartRow label="Wait guard" desc="A new chat refuses a shell command that would wait forever." which="guard" />
    </Group>
  );
}
