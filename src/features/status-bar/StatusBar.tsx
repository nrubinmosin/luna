import { useEffect, useState } from 'react';
import { clockDate, clockTime, clockWeekday } from '../../shared/lib/format';
import { logPath, revealLog } from '../../shared/lib/log';
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
  const runSummary = `${all.filter(c => c.status === 'working').length} working · ${all.filter(c => c.status === 'waiting').length} waiting`;

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
        <div
          className="status-bar-field"
          style={{ fontSize: 'var(--fs-2)', color: 'var(--dim)', whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis' }}
        >
          {runSummary}
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
