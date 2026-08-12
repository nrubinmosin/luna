import { useEffect, useRef, useState } from 'react';
import { clockDate, clockTime, clockWeekday, limitColor } from '../../shared/lib/format';
import { useAccounts } from '../accounts/accounts.store';
import { useChats } from '../chats/chats.store';
import { AccountsPanel } from '../accounts/AccountsPanel';

const LK: Array<['h5' | 'week' | 'fable', string]> = [['h5', '5 hours'], ['week', 'week'], ['fable', 'fable']];

export function StatusBar() {
  const [now, setNow] = useState(() => new Date());
  const accounts = useAccounts(s => s.accounts);
  const open = useAccounts(s => s.open);
  const toggleOpen = useAccounts(s => s.toggleOpen);
  const folders = useChats(s => s.folders);

  const barRef = useRef<HTMLDivElement>(null);
  const leftRef = useRef<HTMLDivElement>(null);
  const [dims, setDims] = useState({ barW: 1200, leftW: 340 });

  useEffect(() => {
    const t = setInterval(() => setNow(new Date()), 1000);
    return () => clearInterval(t);
  }, []);

  useEffect(() => {
    const read = () => {
      const bar = barRef.current, left = leftRef.current;
      if (!bar || !left) return;
      const barW = bar.clientWidth - 24;
      const kids = [...left.children];
      const leftW = kids.reduce((a, k) => a + (k as HTMLElement).scrollWidth, 0) + 12 * Math.max(0, kids.length - 1);
      setDims(d => (d.barW !== barW || d.leftW !== leftW ? { barW, leftW } : d));
    };
    const ro = new ResizeObserver(read);
    if (barRef.current) ro.observe(barRef.current);
    read();
    return () => ro.disconnect();
  }, [accounts.length]);

  const all = folders.flatMap(f => f.chats);
  const runSummary = `${all.filter(c => c.status === 'working').length} working · ${all.filter(c => c.status === 'waiting').length} waiting`;

  const room = Math.floor((dims.barW - dims.leftW - 46) / 220);
  const visible = Math.max(1, Math.min(accounts.length, room));
  const barAccounts = accounts.slice(0, visible);
  const more = accounts.length > visible ? `+${accounts.length - visible}` : '';

  return (
    <div
      ref={barRef}
      style={{
        flex: 'none', height: 38, display: 'flex', alignItems: 'center', gap: 12, padding: '0 12px',
        background: 'var(--sidebar)', borderTop: '1px solid var(--line)', fontSize: 12.5, color: 'var(--dim)', position: 'relative'
      }}
    >
      <div ref={leftRef} style={{ display: 'flex', alignItems: 'center', gap: 12, flex: '0 1 auto', minWidth: 0, overflow: 'hidden' }}>
        <span style={{ fontVariantNumeric: 'tabular-nums', fontWeight: 600, color: 'var(--fg)', fontSize: 13.5, whiteSpace: 'nowrap', flex: 'none' }}>
          {clockTime(now)}
        </span>
        <span style={{ whiteSpace: 'nowrap', flex: 'none' }}>{clockDate(now)}</span>
        <span style={{ color: 'var(--faint)', whiteSpace: 'nowrap', flex: 'none' }}>{clockWeekday(now)}</span>
        <div style={{ width: 1, height: 16, background: 'var(--line)', flex: 'none' }} />
        <span style={{ color: 'var(--faint)', whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis', flex: '0 1 auto', minWidth: 0 }}>
          {runSummary}
        </span>
      </div>

      <div style={{ flex: 1, minWidth: 8 }} />

      <div onClick={toggleOpen} style={{ display: 'flex', alignItems: 'center', gap: 12, cursor: 'default', flex: '0 1 auto', minWidth: 0, overflow: 'hidden' }}>
        {barAccounts.map(acc => (
          <div key={acc.name} style={{ display: 'flex', alignItems: 'center', gap: 8, minWidth: 0, overflow: 'hidden' }}>
            <span
              style={{
                width: 6, height: 6, borderRadius: '50%', flex: 'none',
                background: Math.max(acc.limits.h5, acc.limits.week) >= 0.85 ? 'oklch(.63 .19 25)' : 'oklch(.64 .18 145)'
              }}
            />
            <span style={{ fontSize: 12.5, color: 'var(--fg)', fontWeight: 500, whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis', flex: '0 1 auto', minWidth: 48 }}>
              {acc.name.split('@')[0]}
            </span>
            {LK.map(([k, full]) => (
              <div
                key={k}
                title={`${full}: ${Math.round(acc.limits[k] * 100)}% · resets in ${acc.resets[k]}`}
                style={{ width: 40, height: 6, borderRadius: 3, background: 'var(--track)', border: '1px solid var(--line)', overflow: 'hidden', flex: 'none' }}
              >
                <div style={{ height: '100%', borderRadius: 3, width: `${Math.round(acc.limits[k] * 100)}%`, background: limitColor(acc.limits[k]) }} />
              </div>
            ))}
          </div>
        ))}
        {accounts.length === 0 && (
          <span style={{ fontSize: 12.5, color: 'var(--faint)', whiteSpace: 'nowrap' }}>no accounts — click to add</span>
        )}
        <span style={{ fontSize: 11.5, color: 'var(--faint)', flex: 'none', whiteSpace: 'nowrap' }}>{more}</span>
        <span style={{ fontSize: 11, color: 'var(--faint)', flex: 'none' }}>⌃</span>
      </div>

      {open && <AccountsPanel />}
    </div>
  );
}
