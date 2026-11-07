import { usePanes } from '../../panes/panes.store';
import { Section } from '../../../shared/ui/Section';

/** Whether the board grows and shrinks with the chats on it. */
function AutoTileRow() {
  const on = usePanes(s => s.autoTile);
  return (
    <div className="field-row" style={{ padding: '3px 0' }}>
      <input
        type="checkbox"
        id="auto-tile-toggle"
        checked={on}
        onChange={e => usePanes.getState().setAutoTile(e.target.checked)}
      />
      <label htmlFor="auto-tile-toggle" style={{ cursor: 'default', fontSize: 'var(--fs-4)' }}>
        Auto-tile the board
      </label>
    </div>
  );
}

export function BoardSection() {
  return (
    <Section label="Board">
      <AutoTileRow />
      <div style={{ fontSize: 'var(--fs-1)', color: 'var(--faint)', marginTop: 4 }}>
        A new chat with no free pane grows the board by one pane, up to four, and lands in it; taking a chat off the
        board or deleting it shrinks the board by one. Off, a new chat with nowhere to go comes up on the sheet and the
        board stays as you arranged it.
      </div>
    </Section>
  );
}
