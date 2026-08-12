import { useState } from 'react';
import { limitColor } from '../../shared/lib/format';
import { useAccounts } from './accounts.store';
import { useChats } from '../chats/chats.store';

const LK: Array<['h5' | 'week' | 'fable', string]> = [['h5', '5 hours'], ['week', 'week'], ['fable', 'fable']];

/** Renders inline as the sidebar's account list — no longer a floating popover. */
export function AccountsPanel() {
  const { accounts, adding, error } = useAccounts();
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
        const dot =
          acc.sync === 'error'
            ? 'oklch(.63 .19 25)'
            : acc.sync !== 'ready'
              ? 'var(--faint)'
              : Math.max(acc.limits.h5, acc.limits.week) >= 0.85
                ? 'oklch(.63 .19 25)'
                : 'oklch(.64 .18 145)';
        const note =
          acc.sync === 'loading' ? 'loading…'
          : acc.sync === 'stale' ? 'refreshing token…'
          : acc.sync === 'throttled' ? 'rate-limited'
          : acc.sync === 'error' ? 'unreachable'
          : null;
        return (
          <div key={acc.name} style={{ padding: '6px 0', borderTop: '1px solid var(--line)' }}>
            <div style={{ display: 'flex', alignItems: 'center', gap: 6, marginBottom: 5 }}>
              <span style={{ width: 6, height: 6, borderRadius: '50%', flex: 'none', background: dot }} />
              <span style={{ fontSize: 11.5, fontWeight: 700, whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis' }} title={acc.path}>
                {acc.name}
              </span>
              <span style={{ flex: 1 }} />
              <span style={{ fontSize: 10, color: 'var(--faint)', whiteSpace: 'nowrap' }}>
                {note ?? acc.plan}
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
              <div key={k} title={`${full}: ${Math.round(acc.limits[k] * 100)}% · resets in ${acc.resets[k]}`} style={{ display: 'flex', alignItems: 'center', gap: 5, marginBottom: 3 }}>
                <span style={{ fontSize: 10, color: 'var(--dim)', width: 30, flex: 'none', whiteSpace: 'nowrap', overflow: 'hidden' }}>{full}</span>
                <div className="xp-sunken" style={{ flex: 1, height: 6, background: 'var(--track)', overflow: 'hidden' }}>
                  <div style={{ height: '100%', width: `${Math.round(acc.limits[k] * 100)}%`, background: limitColor(acc.limits[k]) }} />
                </div>
                <span style={{ fontSize: 10, color: 'var(--dim)', width: 26, flex: 'none', textAlign: 'right', fontVariantNumeric: 'tabular-nums' }}>
                  {Math.round(acc.limits[k] * 100)}%
                </span>
              </div>
            ))}
          </div>
        );
      })}
    </div>
  );
}
