import { useEffect, useState } from 'react';
import { clockDate, clockTime, clockWeekday } from '../../shared/lib/format';
import { logPath, revealLog } from '../../shared/lib/log';
import { STATUS } from '../../shared/ui/status';
import { useChats } from '../chats/chats.store';
import { AccountsPanel } from '../accounts/AccountsPanel';
import { UpdateField } from '../updates/UpdateField';

/** Sidebar footer: clock, run summary, log link, then the account list —
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
    <div style={{ display: 'flex', alignItems: 'baseline', gap: 6 }}>
      <span style={{ fontVariantNumeric: 'tabular-nums', fontWeight: 700, color: 'var(--fg)', fontSize: 'var(--fs-4)' }}>
        {clockTime(now)}
      </span>
      <span style={{ fontSize: 'var(--fs-1)', color: 'var(--dim)' }}>{clockDate(now)}</span>
      <span style={{ fontSize: 'var(--fs-1)', color: 'var(--faint)' }}>{clockWeekday(now)}</span>
    </div>
  );
}

export function StatusBar() {
  const folders = useChats(s => s.folders);

  const [logFile, setLogFile] = useState('');
  useEffect(() => {
    void logPath().then(setLogFile).catch(() => {});
  }, []);

  const all = folders.flatMap(f => f.chats);
  const working = all.filter(c => c.status === 'working').length;
  const waiting = all.filter(c => c.status === 'waiting').length;

  return (
    <div style={{ flex: 'none', maxHeight: '58%', display: 'flex', flexDirection: 'column', borderTop: '1px solid var(--window-frame)', background: 'var(--sidebar)' }}>
      <div style={{ flex: '1 1 auto', minHeight: 0, overflowY: 'auto' }}>
        <AccountsPanel />
      </div>

      {/* Bottom edge of the window, where XP puts its status bar. */}
      <div className="status-bar" style={{ flex: 'none' }}>
        <div className="status-bar-field" style={{ flexGrow: 0 }}>
          <Clock />
        </div>
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
        <UpdateField />
        <div
          onClick={() => void revealLog()}
          title={logFile ? `Application log — click to show in Explorer\n${logFile}` : 'Application log'}
          className="status-bar-field hover-bg"
          style={{ flexGrow: 0, fontSize: 'var(--fs-2)', color: 'var(--faint)', cursor: 'default', whiteSpace: 'nowrap' }}
        >
          ▤ log
        </div>
      </div>
    </div>
  );
}
