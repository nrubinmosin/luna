import { useEffect, useState } from 'react';
import { waitingMarker, setWaitingMarker, type WaitingMarker } from '../../../ipc/sessions';
import { CheckRow, Group } from '../SettingsRows';

/** The text a chat's last message ends on when its turn waits for you, as a
 *  reply format of yours writes it. Kept by the Rust side (asking.rs), which
 *  reads it against every turn that ends. */
export function WaitingSection() {
  const [saved, setSaved] = useState<WaitingMarker | null>(null);
  const [text, setText] = useState('');
  useEffect(() => {
    void waitingMarker().then(m => {
      setSaved(m);
      setText(m.text);
    });
  }, []);
  const save = (on: boolean, next: string) =>
    void setWaitingMarker(on, next).then(m => {
      setSaved(m);
      setText(m.text);
    });
  return (
    <Group
      title="Waiting for you"
      about={{
        summary: 'How the waiting marker works',
        body: (
          <p>
            A chat of yours whose turn is over and whose last message ends on a line holding this text shows as waiting,
            as one on a permission prompt does, until you next write to it or clear it by hand with the check on its
            title bar or row; the next turn that ends on it waits again. For a reply format that ends on a status line.
            Helpers ask the chat that started them, not you. A power-off armed for when all is done does not wait for
            such a chat.
          </p>
        )
      }}
    >
      <CheckRow
        label="Waiting marker"
        desc="A chat whose last message ends on a line holding this text shows as waiting for you."
        checked={saved?.on ?? true}
        disabled={saved === null}
        onChange={on => save(on, text)}
        below={
          <div className="set-field">
            <input
              type="text"
              className="set-mono"
              value={text}
              disabled={saved === null || !saved.on}
              onChange={e => setText(e.target.value)}
              onKeyDown={e => {
                if (e.key === 'Enter' && saved) save(saved.on, text);
              }}
              aria-label="Waiting marker text"
            />
            <button
              className="slim"
              disabled={!saved || text.trim() === saved.text}
              onClick={() => saved && save(saved.on, text)}
            >
              Save
            </button>
          </div>
        }
      />
    </Group>
  );
}
