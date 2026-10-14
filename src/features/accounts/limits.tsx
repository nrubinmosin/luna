import { useEffect, useState, type CSSProperties } from 'react';
import { fmtReset, fmtResetDate, limitColor } from '../../shared/lib/format';
import type { Account } from '../../shared/types';

/** The worst of an account's windows, for its dot. */
export const worstLimit = (account: Account) => Math.max(0, ...account.limits.map(w => w.used));

/**
 * A clock for countdowns. The windows carry the instant they reset, not a
 * "3h 40m" worked out when they were fetched — that one went stale between
 * polls and read the same for ten minutes.
 */
export function useNow(everyMs = 30_000) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), everyMs);
    return () => clearInterval(t);
  }, [everyMs]);
  return now;
}

const small: CSSProperties = { fontSize: 'var(--fs-1)', fontVariantNumeric: 'tabular-nums', whiteSpace: 'nowrap', flex: 'none' };

/**
 * Every window of an account as a bar: what it is, how full, when it resets —
 * in so long, and on which date and time exactly.
 */
export function LimitBars({ account }: { account: Account }) {
  const now = useNow();
  const windows = account.haveUsage && account.limits.length
    ? account.limits
    : [{ id: 'none', label: 'usage', short: 'usage', used: 0, resetAt: null }];
  return (
    <div style={{ display: 'grid', gridTemplateColumns: 'auto 1fr auto auto auto', alignItems: 'center', columnGap: 8, rowGap: 4 }}>
      {windows.map(w => {
        const known = account.haveUsage && w.id !== 'none';
        const pct = Math.round(w.used * 100);
        return [
          <span key={`${w.id}-l`} title={w.label} style={{ ...small, color: 'var(--dim)', maxWidth: 110, overflow: 'hidden', textOverflow: 'ellipsis' }}>
            {w.label}
          </span>,
          <span key={`${w.id}-b`} className="meter" style={{ height: 14 }}>
            {known && <i style={{ width: `${pct}%`, ['--m' as string]: limitColor(w.used) } as CSSProperties} />}
          </span>,
          <span key={`${w.id}-p`} style={{ ...small, color: 'var(--fg)', minWidth: 32, textAlign: 'right' }}>{known ? `${pct}%` : '—'}</span>,
          <span key={`${w.id}-r`} style={{ ...small, color: 'var(--dim)', minWidth: 46, textAlign: 'right' }}>
            {known && w.resetAt ? `in ${fmtReset(w.resetAt, now)}` : ''}
          </span>,
          <span key={`${w.id}-d`} style={{ ...small, color: 'var(--faint)', minWidth: 92, textAlign: 'right' }}>
            {known ? fmtResetDate(w.resetAt) ?? '' : ''}
          </span>
        ];
      })}
    </div>
  );
}
