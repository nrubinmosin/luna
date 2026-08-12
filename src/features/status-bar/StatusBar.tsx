import { useEffect, useState } from 'react';
import { clockDate, clockTime, clockWeekday } from '../../shared/lib/format';
import { logPath, revealLog } from '../../shared/lib/log';
import { useChats } from '../chats/chats.store';
import { AccountsPanel } from '../accounts/AccountsPanel';

/** Sidebar footer: clock, run summary, log link, then the account list —
 *  the horizontal top-level status bar this used to be didn't have room for
 *  any of that once panes reclaimed its height. */
export function StatusBar() {
  const [now, setNow] = useState(() => new Date());
  const folders = useChats(s => s.folders);

  const [logFile, setLogFile] = useState('');
  useEffect(() => {
    void logPath().then(setLogFile).catch(() => {});
  }, []);

  useEffect(() => {
    const t = setInterval(() => setNow(new Date()), 1000);
    return () => clearInterval(t);
  }, []);

  const all = folders.flatMap(f => f.chats);
  const runSummary = `${all.filter(c => c.status === 'working').length} working · ${all.filter(c => c.status === 'waiting').length} waiting`;

  return (
    <div style={{ flex: 'none', maxHeight: '58%', display: 'flex', flexDirection: 'column', borderTop: '1px solid var(--window-frame)', background: 'var(--sidebar)' }}>
      <div style={{ flex: 'none', padding: '7px 9px 6px', display: 'flex', flexDirection: 'column', gap: 3 }}>
        <div style={{ display: 'flex', alignItems: 'baseline', gap: 6 }}>
          <span style={{ fontVariantNumeric: 'tabular-nums', fontWeight: 700, color: 'var(--fg)', fontSize: 13 }}>
            {clockTime(now)}
          </span>
          <span style={{ fontSize: 10.5, color: 'var(--dim)' }}>{clockDate(now)}</span>
          <span style={{ fontSize: 10.5, color: 'var(--faint)' }}>{clockWeekday(now)}</span>
        </div>
        <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 6 }}>
          <span style={{ fontSize: 10.5, color: 'var(--dim)', whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis' }}>
            {runSummary}
          </span>
          <span
            onClick={() => void revealLog()}
            title={logFile ? `Application log — click to show in Explorer\n${logFile}` : 'Application log'}
            className="hover-bg"
            style={{
              flex: 'none', height: 15, padding: '0 5px', borderRadius: 2, display: 'flex',
              alignItems: 'center', gap: 3, fontSize: 10, color: 'var(--faint)', cursor: 'default'
            }}
          >
            <span style={{ fontSize: 9 }}>▤</span> log
          </span>
        </div>
      </div>

      <div style={{ flex: '1 1 auto', minHeight: 0, overflowY: 'auto' }}>
        <AccountsPanel />
      </div>
    </div>
  );
}
