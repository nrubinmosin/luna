import { useCore } from './core.store';

/**
 * Said across the top of the board when the core is not there to run the
 * sessions: it stopped (they went with it), one of another build holds them,
 * or Luna runs as administrator, and no session may (corelink.rs). Never
 * restarted behind the user's back — a core that keeps falling over must not
 * hide it — so the way back is this button, where a restart helps: not for a
 * window that is itself elevated, which only starting Luna normally mends.
 */
export function CoreBanner() {
  const link = useCore(s => s.link);
  const restart = useCore(s => s.restart);
  if (link.state !== 'down' && link.state !== 'mismatch' && link.state !== 'elevated') return null;

  const text =
    link.state === 'elevated'
      ? (link.error ?? 'Luna is running as administrator; restart it normally.')
      : link.state === 'mismatch'
        ? `The sessions are running in Luna's core from another build (${link.core?.version ?? '?'}, ${
            link.core?.sessions ?? 0
          } running). Restart it to work with them here: they quit and resume in the new one.`
        : `Luna's core stopped, and the sessions in it ended${link.error ? ` (${link.error})` : ''}. Restart it and the chats resume from their transcripts.`;
  const restartable = link.state !== 'elevated' || link.core != null;
  return (
    <div
      role="alert"
      style={{
        display: 'flex',
        alignItems: 'center',
        gap: 10,
        padding: '6px 10px',
        margin: '6px 6px 0',
        borderRadius: 4,
        background: 'oklch(.55 .17 25 / .18)',
        border: '1px solid oklch(.58 .2 25 / .5)',
        fontSize: 'var(--fs-3)',
        color: 'var(--fg)'
      }}
    >
      <span style={{ flex: 1 }}>{text}</span>
      {restartable && (
        <button className="slim primary" onClick={restart}>
          Restart the core
        </button>
      )}
    </div>
  );
}
