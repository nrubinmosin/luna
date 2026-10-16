import { ConfirmDialog } from '../../shared/ui/ConfirmDialog';
import { LiveSessions } from '../power/LiveSessions';
import { usePower } from '../power/power.store';
import { useCore } from '../core/core.store';
import { keepsSessions, useUpdates } from './updates.store';

/**
 * The release-notes gate in front of an app update. Rendered once at the app
 * root and driven entirely by the store's `asking`, so the status-bar chip and
 * the settings dialog raise the same dialog instead of owning copies of it.
 */
export function UpdateConfirm() {
  const asking = useUpdates(s => s.asking);
  const next = useUpdates(s => s.next);
  const notes = useUpdates(s => s.notes);
  const live = usePower(s => s.state.summary.sessions.length);
  const nextCore = useUpdates(s => s.nextCore);
  // Read so the dialog follows the core; the answer itself is keepsSessions'.
  useCore(s => s.link);
  const keep = keepsSessions(nextCore);

  if (!asking || !next) return null;

  return (
    <ConfirmDialog
      title={`Update to ${next}?`}
      confirmLabel="Download and install"
      danger={false}
      body={
        <>
          The installer is downloaded and checked against this build's signing key, and then
          Luna <b>closes itself</b> — an installer cannot replace the exe of a running app.
          It starts Luna again when it is done.
          {live > 0 && keep ? (
            <div style={{ marginTop: 8 }}>
              This update leaves Luna&apos;s core as it is, so only the window restarts:{' '}
              <b>{live === 1 ? 'the running session keeps' : `the ${live} running sessions keep`}</b> running,
              background work and all, and the new window picks {live === 1 ? 'it' : 'them'} up.
            </div>
          ) : live > 0 ? (
            <div style={{ marginTop: 8 }}>
              Once the download is through, <b>all {live} running session{live === 1 ? '' : 's'}</b>{' '}
              {live === 1 ? 'is' : 'are'} asked to quit, the way deleting a chat does — whatever is
              mid-turn or running in the background stops with {live === 1 ? 'it' : 'them'}. The
              chats resume from their transcripts when Luna is back.
              <LiveSessions />
            </div>
          ) : (
            <div style={{ marginTop: 8 }}>No session is running, so nothing stops.</div>
          )}
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
  );
}
