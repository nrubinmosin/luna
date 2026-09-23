import { useEffect, useState } from 'react';
import { createPortal } from 'react-dom';
import type { CSSProperties } from 'react';
import type { PowerAction } from '../../ipc/commands';
import { useChats, allChats } from '../chats/chats.store';
import { usePower } from './power.store';

const ACTIONS: Array<{ value: PowerAction; label: string; note: string }> = [
  { value: 'shutdown', label: 'Shut down', note: 'closes every session first, then powers off' },
  { value: 'hibernate', label: 'Hibernate', note: 'sessions stay; the rule applies again after the wake' },
  { value: 'sleep', label: 'Sleep', note: 'sessions stay; the rule applies again after the wake' }
];

const QUIET: Array<{ value: number; label: string }> = [
  { value: 60, label: '1 minute' },
  { value: 120, label: '2 minutes' },
  { value: 300, label: '5 minutes' },
  { value: 600, label: '10 minutes' },
  { value: 1800, label: '30 minutes' }
];

/**
 * The window behind the power chip: the keep-awake switch and the
 * when-everything-is-done rule. Same host and frame as the settings dialog.
 */
export function PowerMenu({ onClose }: { onClose: () => void }) {
  const state = usePower(s => s.state);
  const { keepAwake, arm, disarm } = usePower.getState();
  const folders = useChats(s => s.folders);

  const [action, setAction] = useState<PowerAction>(state.armed?.action ?? 'shutdown');
  const [quiet, setQuiet] = useState<number>(state.armed?.quietS ?? 120);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [onClose]);

  const chats = allChats(folders);
  const waiting = chats.filter(c => c.status === 'waiting');
  const worktrees = chats.filter(c => c.worktreePath);
  const armed = state.armed;

  const host = document.querySelector('[data-app]') ?? document.body;
  const label: CSSProperties = { fontSize: 'var(--fs-3)', color: 'var(--dim)', marginBottom: 4, fontWeight: 600 };
  const fine: CSSProperties = { fontSize: 'var(--fs-1)', color: 'var(--faint)', marginTop: 5, lineHeight: 1.45 };

  return createPortal(
    <div
      onClick={e => {
        e.stopPropagation();
        onClose();
      }}
      style={{
        position: 'fixed', inset: 0, background: 'rgba(0,0,0,.35)', backdropFilter: 'blur(1px)',
        display: 'grid', placeItems: 'center', zIndex: 70
      }}
    >
      <div
        onClick={e => e.stopPropagation()}
        className="window"
        style={{ width: 430, boxShadow: 'var(--shadow), var(--border-window-outer), var(--border-window-inner)' }}
      >
        <div className="title-bar">
          <div className="title-bar-text">Power</div>
          <div className="title-bar-controls">
            <button aria-label="Close" onClick={onClose} />
          </div>
        </div>
        <div className="window-body">
          <div style={{ marginBottom: 14 }}>
            <div className="field-row" style={{ height: 'calc(var(--ui) * 1.6)' }}>
              <input
                type="checkbox"
                id="keep-awake"
                checked={state.keepAwake}
                onChange={() => void keepAwake(!state.keepAwake)}
              />
              <label htmlFor="keep-awake" style={{ cursor: 'default' }}>
                Keep the PC awake while sessions work
              </label>
            </div>
            <div style={fine}>
              Held while any session is mid-turn, has something running in its shell, or is still
              printing; let go a minute after the last one goes quiet. A session waiting for your
              answer does not hold it. Shows up in <code>powercfg /requests</code> as Luna's.
              {state.holding ? ' Holding now.' : ''}
            </div>
          </div>

          <div style={{ marginBottom: 14 }}>
            <div style={label}>When every session is done</div>
            <div style={{ display: 'flex', gap: 8 }}>
              <select value={action} onChange={e => setAction(e.target.value as PowerAction)} disabled={!!armed} style={{ flex: 1 }}>
                {ACTIONS.map(a => (
                  <option key={a.value} value={a.value}>{a.label}</option>
                ))}
              </select>
              <select value={quiet} onChange={e => setQuiet(Number(e.target.value))} disabled={!!armed} style={{ width: 130 }}>
                {QUIET.map(q => (
                  <option key={q.value} value={q.value}>after {q.label}</option>
                ))}
              </select>
            </div>
            <div style={fine}>
              {ACTIONS.find(a => a.value === action)?.note}. Done means: no session mid-turn or
              waiting for you, nothing running under any of them, screens quiet — for the whole
              window. Then a one-minute countdown with a notification; any activity in it cancels
              the count and keeps the rule armed.
            </div>
            {waiting.length > 0 && (
              <div style={{ ...fine, color: 'var(--dim)' }}>
                Waiting for you now: {waiting.map(c => c.name).join(', ')} — the rule will not fire
                until they are answered.
              </div>
            )}
            {worktrees.length > 0 && (
              <div style={{ ...fine, color: 'var(--dim)' }}>
                In worktrees: {worktrees.map(c => c.name).join(', ')}. Anything uncommitted there
                stays on disk, but check before you shut down.
              </div>
            )}
          </div>

          <div style={{ display: 'flex', justifyContent: 'flex-end', gap: 8 }}>
            {armed ? (
              <button onClick={() => void disarm()} className="danger">Disarm</button>
            ) : (
              <button onClick={() => void arm(action, quiet)} className="primary">Arm</button>
            )}
            <button onClick={onClose}>Close</button>
          </div>
        </div>
      </div>
    </div>,
    host
  );
}
