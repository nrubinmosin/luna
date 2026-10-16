import { allChats, useChats } from '../chats/chats.store';
import { usePower } from './power.store';
import type { SessionActivityDto } from '../../ipc/commands';

/** What a running session is doing, in the words the power chip uses. */
const doing = (s: SessionActivityDto): string => {
  if (s.turn === 'waiting' && !s.busy) return s.stalled ? `stopped on ${s.stalled}` : 'waiting for you';
  const why = [
    s.turn === 'busy' ? 'mid-turn' : '',
    s.procs.length ? `running ${s.procs.join(', ')}` : '',
    s.outputFresh && s.turn !== 'busy' ? 'printing' : ''
  ].filter(Boolean);
  return why.length ? why.join(', ') : 'idle';
};

/**
 * Every running session with what it is doing, for the dialogs whose confirm
 * stops them all — an update and Quit. The busy ones come first and stand
 * out: those are the ones that lose work. A Claude subagent left running in
 * the background after its turn ended is not a process of its own and reads
 * as idle here, which is why the dialogs speak of every session, not only of
 * the busy ones.
 */
export function LiveSessions() {
  const sessions = usePower(s => s.state.summary.sessions);
  const folders = useChats(s => s.folders);
  if (!sessions.length) return null;

  const name = (id: string) => allChats(folders).find(c => c.id === id)?.name ?? id.slice(0, 8);
  const rows = [...sessions].sort((a, b) => Number(b.busy) - Number(a.busy));
  return (
    <ul style={{ margin: '6px 0 0', paddingLeft: 18, maxHeight: 140, overflowY: 'auto' }}>
      {rows.map(s => (
        <li key={s.id} style={s.busy ? { fontWeight: 600 } : { color: 'var(--dim)' }}>
          {name(s.id)} — {doing(s)}
        </li>
      ))}
    </ul>
  );
}
