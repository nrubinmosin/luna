import { useNotifyPrefs } from '../../../shared/lib/notifyPrefs.store';
import { CheckRow, Group } from '../SettingsRows';

/** Whether desktop notifications play the system's sound. */
export function NotificationsSection() {
  const on = useNotifyPrefs(s => s.sound);
  return (
    <Group
      title="Notifications"
      about={{
        summary: 'How notifications work',
        body: (
          <p>
            The system’s own notification sound, at its volume and muted by Do Not Disturb like any other. A click on a
            notification about a chat — waiting for you, or one its agent raised with <code>luna_notify</code> — brings
            that chat up on the peek sheet.
          </p>
        )
      }}
    >
      <CheckRow
        label="Play a sound"
        desc="Desktop notifications play the system’s notification sound."
        checked={on}
        onChange={v => useNotifyPrefs.getState().setSound(v)}
      />
    </Group>
  );
}
