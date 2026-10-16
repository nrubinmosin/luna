import { useEffect, useState } from 'react';
import { ConfirmDialog } from '../shared/ui/ConfirmDialog';
import { LiveSessions } from '../features/power/LiveSessions';
import { onQuitAsked } from '../ipc/events';
import { quitApp, quitDeclined } from '../ipc/commands';

/**
 * The question in front of a tray Quit while sessions are running: they all
 * stop with Luna. Rust asks only when there is something to lose, and quits
 * without asking on a second Quit while this is still up (lifecycle.rs).
 */
export function QuitConfirm() {
  const [live, setLive] = useState(0);

  useEffect(() => {
    const off = onQuitAsked(setLive);
    return () => void off.then(f => f());
  }, []);

  if (!live) return null;

  return (
    <ConfirmDialog
      title="Quit Luna?"
      confirmLabel="Stop sessions and quit"
      body={
        <>
          <b>{live === 1 ? 'The running session' : `All ${live} running sessions`}</b> will be asked
          to quit, the way deleting a chat does, and whatever is mid-turn or running in the
          background stops with {live === 1 ? 'it' : 'them'}. The chats resume from their
          transcripts the next time Luna starts.
          <LiveSessions />
        </>
      }
      onConfirm={() => {
        setLive(0);
        void quitApp();
      }}
      onCancel={() => {
        setLive(0);
        void quitDeclined();
      }}
    />
  );
}
