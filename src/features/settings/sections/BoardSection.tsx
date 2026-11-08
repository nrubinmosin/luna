import { usePanes } from '../../panes/panes.store';
import { CheckRow, Group } from '../SettingsRows';

/** Whether the board grows and shrinks with the chats on it. */
export function BoardSection() {
  const on = usePanes(s => s.autoTile);
  return (
    <Group
      title="Board"
      about={{
        summary: 'How auto-tiling works',
        body: (
          <p>
            A new chat with no free pane grows the board by one pane, up to four, and lands in it; taking a chat off the
            board or deleting it shrinks the board by one. Off, a new chat with nowhere to go comes up on the sheet and
            the board stays as you arranged it.
          </p>
        )
      }}
    >
      <CheckRow
        label="Auto-tile the board"
        desc="Grow and shrink the board with the chats on it, up to four panes."
        checked={on}
        onChange={v => usePanes.getState().setAutoTile(v)}
      />
    </Group>
  );
}
