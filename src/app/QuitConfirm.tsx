import { useEffect, useState } from 'react';
import { ConfirmDialog } from '../shared/ui/ConfirmDialog';
import { LiveSessions } from '../features/power/LiveSessions';
import { onQuitAsked } from '../ipc/events';
import { quitApp, quitDeclined } from '../ipc/commands';

/**
 * The question in front of a tray Quit while sessions are running. They live
 * in the core, not the window, so by default they keep running and the next
 * window picks them up; stopping them is the opt-in. Rust asks only when
 * there is something running, and quits without asking — sessions left be —
 * on a second Quit while this is still up (lifecycle.rs).
 */
export function QuitConfirm() {
  const [live, setLive] = useState(0);
  const [stop, setStop] = useState(false);

  useEffect(() => {
    const off = onQuitAsked(n => {
      setStop(false);
      setLive(n);
    });
    return () => void off.then(f => f());
  }, []);

  if (!live) return null;

  const them = live === 1 ? 'it' : 'them';
  return (
    <ConfirmDialog
      title="Quit Luna?"
      danger={stop}
      confirmLabel={stop ? 'Stop sessions and quit' : 'Quit'}
      body={
        stop ? (
          <>
            <b>{live === 1 ? 'The running session' : `All ${live} running sessions`}</b> will be asked to
            quit, the way deleting a chat does, and whatever is mid-turn or running in the background
            stops with {them}. The chats resume from their transcripts the next time Luna starts.
            <LiveSessions />
          </>
        ) : (
          <>
            The window closes; <b>{live === 1 ? 'the running session keeps' : `the ${live} running sessions keep`}</b>{' '}
            running in Luna&apos;s core, background work and all, and the next window picks {them} up where{' '}
            {live === 1 ? 'it is' : 'they are'}.
            <LiveSessions />
          </>
        )
      }
      extra={
        // xp.css draws the box on a label tied to the input by id.
        <div style={{ display: 'flex', alignItems: 'center', gap: 7, fontSize: 'var(--fs-4)', color: 'var(--dim)' }}>
          <input type="checkbox" id="quit-stop-sessions" checked={stop} onChange={e => setStop(e.target.checked)} />
          <label htmlFor="quit-stop-sessions">Stop the sessions too</label>
        </div>
      }
      onConfirm={() => {
        setLive(0);
        void quitApp(stop);
      }}
      onCancel={() => {
        setLive(0);
        void quitDeclined();
      }}
    />
  );
}
