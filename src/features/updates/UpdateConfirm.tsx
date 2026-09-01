import { ConfirmDialog } from '../../shared/ui/ConfirmDialog';
import { useUpdates } from './updates.store';

/**
 * The release-notes gate in front of an app update. Rendered once at the app
 * root and driven entirely by the store's `asking`, so the status-bar chip and
 * the settings dialog raise the same dialog instead of owning copies of it.
 */
export function UpdateConfirm() {
  const asking = useUpdates(s => s.asking);
  const next = useUpdates(s => s.next);
  const notes = useUpdates(s => s.notes);

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
  );
}
