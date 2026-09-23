import { useEffect, useState } from 'react';
import { ACCENT } from '../../shared/lib/format';
import { useChats, allChats } from '../chats/chats.store';
import { usePower } from './power.store';
import { PowerMenu } from './PowerMenu';

const chipStyle = (loud: boolean) => ({
  flexGrow: 0,
  fontSize: 'var(--fs-2)',
  cursor: 'default',
  whiteSpace: 'nowrap' as const,
  fontVariantNumeric: 'tabular-nums',
  color: loud ? ACCENT : 'var(--dim)',
  fontWeight: loud ? 700 : 400
});

const VERB = { shutdown: 'shutdown', hibernate: 'hibernate', sleep: 'sleep', log: 'log' } as const;

/**
 * The footer's word about the machine: a power symbol to open the menu, and
 * while a power-off is armed or counting down, that instead. The tooltip
 * says what is holding it — which chats, and what is running under them.
 */
export function PowerChip() {
  const state = usePower(s => s.state);
  const open = usePower(s => s.open);
  const folders = useChats(s => s.folders);
  const [now, setNow] = useState(() => Date.now());

  useEffect(() => {
    void usePower.getState().init();
  }, []);

  // A ticking countdown needs a clock; nothing else here does.
  useEffect(() => {
    if (!state.countdownEndsAtMs) return;
    const t = setInterval(() => setNow(Date.now()), 500);
    return () => clearInterval(t);
  }, [state.countdownEndsAtMs]);

  const { summary, armed, countdownEndsAtMs, holding } = state;
  const name = (id: string) => allChats(folders).find(c => c.id === id)?.name ?? id.slice(0, 8);
  const blockers = summary.sessions
    .filter(s => s.busy || s.turn === 'waiting')
    .map(s => {
      const why = s.turn === 'waiting' && !s.busy
        ? 'waiting for you'
        : [
            s.turn === 'busy' ? 'mid-turn' : '',
            s.procs.length ? `running ${s.procs.join(', ')}` : '',
            s.outputFresh && s.turn !== 'busy' ? 'printing' : ''
          ].filter(Boolean).join(', ');
      return `${name(s.id)} — ${why}`;
    });

  let text: string;
  let title: string;
  let loud = false;
  if (countdownEndsAtMs) {
    const left = Math.max(0, Math.ceil((countdownEndsAtMs - now) / 1000));
    text = `⏻ ${VERB[armed?.action ?? 'shutdown']} in ${left}s`;
    title = 'Every session is done. Click to cancel.';
    loud = true;
  } else if (armed) {
    text = `⏻ ${VERB[armed.action]} when done`;
    title = blockers.length
      ? `Waiting on:\n${blockers.join('\n')}`
      : `All quiet — ${VERB[armed.action]} once it stays that way for ${armed.quietS / 60} min. Click to change or disarm.`;
    loud = true;
  } else {
    text = holding ? '⏻ awake' : '⏻';
    title = holding
      ? `The PC is held awake by:\n${blockers.join('\n') || 'a session that just went quiet'}\nClick for the power rules.`
      : 'Power: keep awake, shut down when done. Click to set up.';
  }

  return (
    <>
      <div
        onClick={() => {
          if (countdownEndsAtMs) void usePower.getState().disarm();
          else usePower.getState().setOpen(true);
        }}
        title={title}
        className="status-bar-field hover-bg"
        style={chipStyle(loud)}
      >
        {text}
      </div>
      {open && <PowerMenu onClose={() => usePower.getState().setOpen(false)} />}
    </>
  );
}
