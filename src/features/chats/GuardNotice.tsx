import { allChats, useChats } from './chats.store';

/**
 * Said once, across the top of the board, to whoever had chats with Luna's
 * tools from before the wait guard was each chat's choice: those chats had
 * it without asking, and now start without it (chats.store.ts `decideGuard`).
 * Gone once dismissed; each chat's ⓘ panel turns it back on.
 */
export function GuardNotice() {
  const ids = useChats(s => s.guardNotice);
  const folders = useChats(s => s.folders);
  const names = allChats(folders)
    .filter(c => ids.includes(c.id))
    .map(c => c.name);
  if (names.length === 0) return null;
  const shown = names.length > 3 ? `${names.slice(0, 3).join(', ')} and ${names.length - 3} more` : names.join(', ');
  const one = names.length === 1;
  return (
    <div
      role="status"
      style={{
        display: 'flex',
        alignItems: 'center',
        gap: 10,
        padding: '6px 10px',
        margin: '6px 6px 0',
        borderRadius: 4,
        background: 'oklch(.7 .12 80 / .16)',
        border: '1px solid oklch(.62 .14 75 / .5)',
        fontSize: 'var(--fs-3)',
        color: 'var(--fg)'
      }}
    >
      <span style={{ flex: 1 }}>
        The wait guard is now each chat&apos;s choice, off unless asked for. {shown} had it because of Luna tools, and
        no longer {one ? 'refuses' : 'refuse'} a command that waits forever. Turn it back on in {one ? 'its' : 'a'}{' '}
        chat&apos;s ⓘ (What Luna applied).
      </span>
      <button className="slim" onClick={() => useChats.getState().dismissGuardNotice()}>
        Dismiss
      </button>
    </div>
  );
}
