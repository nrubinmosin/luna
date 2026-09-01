import { useEffect, useState } from 'react';
import { clockDate, clockTime, clockWeekday, ACCENT } from '../../shared/lib/format';
import { STATUS } from '../../shared/ui/status';
import { useChats } from '../chats/chats.store';
import { AccountsPanel } from '../accounts/AccountsPanel';
import { useUpdates } from '../updates/updates.store';
import { useCli } from '../updates/cli.store';
import { cliUpdateNow } from '../../ipc/commands';

/** Sidebar footer: clock, run summary, then the account list —
 *  the horizontal top-level status bar this used to be didn't have room for
 *  any of that once panes reclaimed its height. */
/** Its own component so the once-a-second tick re-renders three spans rather
 *  than the whole footer, account rows and limit bars included. */
function Clock() {
  const [now, setNow] = useState(() => new Date());
  useEffect(() => {
    const t = setInterval(() => setNow(new Date()), 1000);
    return () => clearInterval(t);
  }, []);

  return (
    // nowrap keeps the bar's height still: "August 17" breaking into two
    // lines once a squeeze got narrow enough made the whole footer jump.
    <div style={{ display: 'flex', alignItems: 'baseline', gap: 6, whiteSpace: 'nowrap' }}>
      <span style={{ fontVariantNumeric: 'tabular-nums', fontWeight: 700, color: 'var(--fg)', fontSize: 'var(--fs-4)' }}>
        {clockTime(now)}
      </span>
      <span style={{ fontSize: 'var(--fs-1)', color: 'var(--dim)' }}>{clockDate(now)}</span>
      <span style={{ fontSize: 'var(--fs-1)', color: 'var(--faint)' }}>{clockWeekday(now)}</span>
    </div>
  );
}

const chipStyle = (loud: boolean) => ({
  flexGrow: 0,
  fontSize: 'var(--fs-2)',
  cursor: 'default',
  whiteSpace: 'nowrap' as const,
  fontVariantNumeric: 'tabular-nums',
  color: loud ? ACCENT : 'var(--dim)',
  fontWeight: loud ? 700 : 400
});

const pct = (got: number, total: number | null) =>
  total ? Math.min(100, Math.round((got / total) * 100)) : null;

/**
 * The bar's only word about updates, and only while there is news: a release
 * on offer, a download running, or a failure worth a retry. The quiet version
 * readouts that used to sit here full-time wrapped the bar onto a second
 * ragged line — they live in Settings now, where clicking through to them is
 * never urgent.
 */
function UpdateChips() {
  const { phase, next, got, total } = useUpdates();
  const cli = useCli(s => s.status);

  const chips = [];

  if (phase === 'available') {
    chips.push(
      <div
        key="app"
        onClick={() => useUpdates.getState().setAsking(true)}
        title={`Version ${next} is out — click for the release notes`}
        className="status-bar-field hover-bg"
        style={chipStyle(true)}
      >
        {`↑ ${next}`}
      </div>
    );
  } else if (phase === 'downloading') {
    const done = pct(got, total);
    chips.push(
      <div
        key="app"
        title={`Downloading ${next}… Luna closes on its own to install it.`}
        className="status-bar-field"
        style={chipStyle(true)}
      >
        {`↓ ${done == null ? '…' : `${done}%`}`}
      </div>
    );
  } else if (phase === 'error') {
    chips.push(
      <div
        key="app"
        onClick={() => void useUpdates.getState().check(true)}
        title="Update check failed — click to try again"
        className="status-bar-field hover-bg"
        style={chipStyle(false)}
      >
        ⚠ update
      </div>
    );
  }

  if (cli?.phase === 'downloading') {
    const done = pct(cli.got, cli.total);
    // The first download is the one that matters — without it a fresh install
    // has nothing to run; it is the only CLI state drawn loud.
    chips.push(
      <div
        key="cli"
        title={`Downloading Claude Code ${cli.latest ?? ''}… new chats use it once it lands.`}
        className="status-bar-field"
        style={chipStyle(!cli.version)}
      >
        {`cli ↓ ${done == null ? '…' : `${done}%`}`}
      </div>
    );
  } else if (cli?.phase === 'error') {
    chips.push(
      <div
        key="cli"
        onClick={() => void cliUpdateNow()}
        title={`Claude CLI update failed — click to try again\n${cli.error ?? ''}`}
        className="status-bar-field hover-bg"
        style={chipStyle(false)}
      >
        cli ⚠
      </div>
    );
  }

  return <>{chips}</>;
}

export function StatusBar() {
  const folders = useChats(s => s.folders);

  const all = folders.flatMap(f => f.chats);
  const working = all.filter(c => c.status === 'working').length;
  const waiting = all.filter(c => c.status === 'waiting').length;

  return (
    <div style={{ flex: 'none', maxHeight: '58%', display: 'flex', flexDirection: 'column', borderTop: '1px solid var(--window-frame)', background: 'var(--sidebar)' }}>
      <div style={{ flex: '1 1 auto', minHeight: 0, overflowY: 'auto' }}>
        <AccountsPanel />
      </div>

      {/* Bottom edge of the window, where XP puts its status bar. One row for
          good: clock, sometimes an update chip, and the two counts. The date
          clips first when the sidebar is dragged narrow — better a shorter
          date than the two-line wrap this bar used to fall into. */}
      <div className="status-bar" style={{ flex: 'none' }}>
        {/* Takes the slack on its row, so the fields after it sit against the
            right edge instead of trailing off into dead space. */}
        <div className="status-bar-field" style={{ flexGrow: 1, minWidth: 0, overflow: 'hidden' }}>
          <Clock />
        </div>
        <UpdateChips />
        {/* Two counts and nothing else: the field is the narrowest strip in the
            window, and spelled-out labels only got themselves ellipsised away. */}
        <div
          className="status-bar-field"
          title={`${working} working · ${waiting} waiting for you`}
          style={{
            flexGrow: 0, display: 'flex', alignItems: 'center', gap: 7,
            fontSize: 'var(--fs-3)', fontVariantNumeric: 'tabular-nums', whiteSpace: 'nowrap'
          }}
        >
          <span style={{ display: 'flex', alignItems: 'center', gap: 3, color: working ? STATUS.working.color : 'var(--faint)' }}>
            <span style={{ width: 6, height: 6, borderRadius: '50%', background: 'currentColor' }} />
            {working}
          </span>
          <span style={{ display: 'flex', alignItems: 'center', gap: 3, color: waiting ? STATUS.waiting.color : 'var(--faint)' }}>
            <span style={{ width: 6, height: 6, borderRadius: '50%', background: 'currentColor' }} />
            {waiting}
          </span>
        </div>
      </div>
    </div>
  );
}
