import { useNotifyPrefs } from '../../../shared/lib/notifyPrefs.store';
import { Section } from '../../../shared/ui/Section';

/** Whether desktop notifications play the system's sound. */
function SoundRow() {
  const on = useNotifyPrefs(s => s.sound);
  return (
    <div className="field-row" style={{ padding: '3px 0' }}>
      <input
        type="checkbox"
        id="notify-sound-toggle"
        checked={on}
        onChange={e => useNotifyPrefs.getState().setSound(e.target.checked)}
      />
      <label htmlFor="notify-sound-toggle" style={{ cursor: 'default', fontSize: 'var(--fs-4)' }}>
        Play a sound
      </label>
    </div>
  );
}

export function NotificationsSection() {
  return (
    <Section label="Notifications">
      <SoundRow />
      <div style={{ fontSize: 'var(--fs-1)', color: 'var(--faint)', marginTop: 4 }}>
        The system’s own notification sound, at its volume and muted by Do Not Disturb like any other. A click on a
        notification about a chat — waiting for you, or one its agent raised with <code>luna_notify</code> — brings that
        chat up on the peek sheet.
      </div>
    </Section>
  );
}
