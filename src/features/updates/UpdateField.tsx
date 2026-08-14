import { ACCENT } from '../../shared/lib/format';
import { ConfirmDialog } from '../../shared/ui/ConfirmDialog';
import { useUpdates } from './updates.store';

const pct = (got: number, total: number | null) =>
  total ? Math.min(100, Math.round((got / total) * 100)) : null;

const ago = (at: number | null) => {
  if (!at) return 'never';
  const m = Math.round((Date.now() - at) / 60000);
  if (m < 1) return 'just now';
  if (m < 60) return `${m}m ago`;
  return `${Math.round(m / 60)}h ago`;
};

/**
 * The update control, in the status bar next to the log link. One field that
 * doubles as the version readout: it says which build is running until there is
 * a newer one to say instead.
 */
export function UpdateField() {
  // The whole state, deliberately: every field here is drawn, and the download
  // re-renders this on every chunk regardless.
  const { phase, current, next, notes, got, total, error, checkedAt, asking } = useUpdates();

  const done = pct(got, total);
  const label =
    phase === 'checking'
      ? 'checking…'
      : phase === 'available'
        ? `↑ ${next}`
        : phase === 'downloading'
          ? `↓ ${done == null ? '…' : `${done}%`}`
          : phase === 'error'
            ? '⚠ update'
            : current
              ? `v${current}`
              : '↑ check';

  const title =
    phase === 'available'
      ? `Version ${next} is out — click for the release notes`
      : phase === 'downloading'
        ? `Downloading ${next}… Luna closes on its own to install it.`
        : phase === 'error'
          ? `Update check failed — click to try again\n${error ?? ''}`
          : `Luna ${current || '—'} · checked ${ago(checkedAt)}\nClick to check for updates`;

  // Only the state that carries news is coloured; the version readout is
  // furniture and reads as quietly as the log link beside it.
  const loud = phase === 'available';

  const onClick = () => {
    const s = useUpdates.getState();
    if (phase === 'available') s.setAsking(true);
    else if (phase !== 'checking' && phase !== 'downloading') void s.check(true);
  };

  return (
    <>
      <div
        onClick={onClick}
        title={title}
        className="status-bar-field hover-bg"
        style={{
          flexGrow: 0, fontSize: 'var(--fs-2)', cursor: 'default', whiteSpace: 'nowrap',
          color: loud ? ACCENT : 'var(--faint)', fontWeight: loud ? 700 : 400
        }}
      >
        {label}
      </div>

      {asking && next && (
        <ConfirmDialog
          title={`Update to ${next}?`}
          confirmLabel="Download and install"
          danger={false}
          body={
            <>
              The installer is downloaded and checked against this build's signing key, and then
              Luna <b>closes itself</b> — an installer cannot replace the exe of a running app.
              It starts Luna again when it is done.
              <div style={{ marginTop: 8 }}>
                Every session goes down with the app, mid-turn and without being asked. The
                transcripts stay on disk, so the chats resume when it comes back — and any session
                that outlives the app turns up under “Sessions with no chat”.
              </div>
              {notes && (
                <div
                  style={{
                    marginTop: 8, padding: '5px 7px', borderRadius: 2, background: 'var(--chip)',
                    fontSize: 'var(--fs-3)', color: 'var(--dim)', maxHeight: 140, overflowY: 'auto',
                    whiteSpace: 'pre-wrap'
                  }}
                >
                  {notes}
                </div>
              )}
            </>
          }
          onConfirm={() => void useUpdates.getState().install()}
          onCancel={() => useUpdates.getState().setAsking(false)}
        />
      )}
    </>
  );
}
