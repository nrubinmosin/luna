import { useState } from 'react';
import { limitColor } from '../../shared/lib/format';
import { useAccounts } from './accounts.store';
import { useChats } from '../chats/chats.store';

const LK: Array<['h5' | 'week' | 'fable', string]> = [['h5', '5 hours'], ['week', 'week'], ['fable', 'fable']];

/** Renders inline as the sidebar's account list — no longer a floating popover. */
export function AccountsPanel() {
  const accounts = useAccounts(s => s.accounts);
  const adding = useAccounts(s => s.adding);
  const error = useAccounts(s => s.error);
  const { add, remove, setAdding, setLoginFor } = useAccounts.getState();
  const [name, setName] = useState('');
  const folders = useChats(s => s.folders);

  const inUse = (account: string) =>
    folders.some(f => f.chats.some(c => c.account === account));

  return (
    <div style={{ borderTop: '1px solid var(--line)', padding: '7px 9px 9px', overflowY: 'auto' }}>
      <div style={{ display: 'flex', alignItems: 'center', marginBottom: 6 }}>
        <div style={{ fontSize: 10.5, fontWeight: 700, textTransform: 'uppercase', letterSpacing: '.04em', color: 'var(--faint)' }}>
          Accounts
        </div>
        <span style={{ flex: 1 }} />
        <div
          onClick={() => setAdding(!adding)}
          title="Add account — creates Documents/claude-accounts/<name>"
          className="hover-bg xp-raised"
          style={{ height: 17, padding: '0 6px', borderRadius: 2, border: '1px solid var(--window-frame)', background: 'var(--panel)', display: 'flex', alignItems: 'center', gap: 3, fontSize: 10.5, cursor: 'default' }}
        >
          <span style={{ fontSize: 11, lineHeight: 1 }}>+</span> add
        </div>
      </div>

      {adding && (
        <div style={{ display: 'flex', gap: 4, marginBottom: 8 }}>
          <input
            autoFocus
            value={name}
            onChange={e => setName(e.target.value)}
            onKeyDown={e => {
              if (e.key === 'Enter' && name.trim()) {
                void add(name);
                setName('');
              }
              if (e.key === 'Escape') setAdding(false);
            }}
            placeholder="account name"
            className="xp-field"
            style={{
              flex: 1, height: 21, padding: '0 5px', border: '1px solid var(--input-border-color)', borderRadius: 1,
              background: '#fff', color: '#000', font: 'inherit', fontSize: 11, outline: 'none'
            }}
          />
          <div
            onClick={() => {
              if (name.trim()) {
                void add(name);
                setName('');
              }
            }}
            className="hover-bright"
            style={{ height: 21, padding: '0 8px', borderRadius: 2, border: '1px solid var(--window-frame)', background: 'var(--accent)', color: '#fff', display: 'flex', alignItems: 'center', fontSize: 10.5, fontWeight: 700, cursor: 'default' }}
          >
            Create
          </div>
        </div>
      )}
      {error && <div style={{ fontSize: 11, color: 'oklch(.58 .2 25)', marginBottom: 6 }}>{error}</div>}

      {accounts.length === 0 && (
        <div style={{ fontSize: 11, color: 'var(--faint)', padding: '2px 0' }}>
          No accounts yet — add one to spawn sessions.
        </div>
      )}

      {accounts.map(acc => {
        // The dot answers "can this account be used", which is the sign-in
        // state — not whether the usage endpoint happened to answer.
        const dot = !acc.signedIn
          ? 'oklch(.63 .19 25)'
          : acc.sync === 'loading'
            ? 'var(--faint)'
            : Math.max(acc.limits.h5, acc.limits.week) >= 0.85
              ? 'oklch(.63 .19 25)'
              : 'oklch(.64 .18 145)';
        const note = !acc.signedIn
          ? 'signed out'
          : acc.sync === 'loading' && !acc.haveUsage
            ? 'loading…'
            : acc.plan;
        // Usage can lag behind sign-in; say so beside the bars, not instead of
        // the plan, so a throttled endpoint never reads as a broken account.
        const usageNote = !acc.haveUsage
          ? acc.sync === 'stale'
            ? 'waiting for token refresh'
            : acc.sync === 'throttled'
              ? 'usage rate-limited'
              : acc.sync === 'error'
                ? 'usage unavailable'
                : 'no usage data yet'
          : acc.usageAge;
        return (
          <div key={acc.name} style={{ padding: '6px 0', borderTop: '1px solid var(--line)' }}>
            <div style={{ display: 'flex', alignItems: 'center', gap: 6, marginBottom: 5 }}>
              <span style={{ width: 6, height: 6, borderRadius: '50%', flex: 'none', background: dot }} />
              <span
                style={{ fontSize: 11.5, fontWeight: 700, whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis' }}
                title={[acc.email, acc.path].filter(Boolean).join('\n')}
              >
                {acc.name}
              </span>
              <span style={{ flex: 1 }} />
              <span
                title={acc.signedIn ? 'Signed in' : 'No usable credentials — click login'}
                style={{
                  fontSize: 10, whiteSpace: 'nowrap',
                  color: acc.signedIn ? 'var(--faint)' : 'oklch(.58 .2 25)'
                }}
              >
                {note}
              </span>
              <span
                onClick={() => setLoginFor(acc)}
                title="Sign in / re-login this account"
                className="hover-bg"
                style={{ height: 15, padding: '0 5px', flex: 'none', borderRadius: 2, display: 'grid', placeItems: 'center', fontSize: 10, color: 'var(--dim)', background: 'var(--chip)', cursor: 'default' }}
              >
                login
              </span>
              <span
                onClick={() => {
                  if (inUse(acc.name) && !window.confirm(`"${acc.name}" is used by existing chats. Delete its folder anyway?`)) return;
                  void remove(acc.name);
                }}
                title="Delete account folder"
                className="hover-danger"
                style={{ width: 15, height: 15, flex: 'none', borderRadius: 2, display: 'grid', placeItems: 'center', fontSize: 10, color: 'var(--faint)', cursor: 'default' }}
              >
                ✕
              </span>
            </div>
            {LK.map(([k, full]) => (
              <div
                key={k}
                title={
                  acc.haveUsage
                    ? `${full}: ${Math.round(acc.limits[k] * 100)}% · resets in ${acc.resets[k]}`
                    : usageNote ?? ''
                }
                style={{ display: 'flex', alignItems: 'center', gap: 5, marginBottom: 3 }}
              >
                <span style={{ fontSize: 10, color: 'var(--dim)', width: 30, flex: 'none', whiteSpace: 'nowrap', overflow: 'hidden' }}>{full}</span>
                <div className="xp-sunken" style={{ flex: 1, height: 6, background: 'var(--track)', overflow: 'hidden' }}>
                  {acc.haveUsage && (
                    <div style={{ height: '100%', width: `${Math.round(acc.limits[k] * 100)}%`, background: limitColor(acc.limits[k]) }} />
                  )}
                </div>
                <span style={{ fontSize: 10, color: 'var(--dim)', width: 26, flex: 'none', textAlign: 'right', fontVariantNumeric: 'tabular-nums' }}>
                  {acc.haveUsage ? `${Math.round(acc.limits[k] * 100)}%` : '—'}
                </span>
              </div>
            ))}
            {usageNote && (
              <div style={{ fontSize: 9.5, color: 'var(--faint)', textAlign: 'right', marginTop: 1 }}>
                {usageNote}
              </div>
            )}
          </div>
        );
      })}
    </div>
  );
}
