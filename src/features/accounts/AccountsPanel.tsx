import { useState } from 'react';
import { limitColor } from '../../shared/lib/format';
import { useAccounts } from './accounts.store';
import { useChats } from '../chats/chats.store';

const LK: Array<['h5' | 'week' | 'fable', string]> = [['h5', '5 hours'], ['week', 'week'], ['fable', 'fable']];

export function AccountsPanel() {
  const { accounts, adding, error } = useAccounts();
  const { add, remove, setAdding, setLoginFor } = useAccounts.getState();
  const [name, setName] = useState('');
  const folders = useChats(s => s.folders);

  const inUse = (account: string) =>
    folders.some(f => f.chats.some(c => c.account === account));

  return (
    <div style={{ position: 'absolute', right: 12, bottom: 44, width: 360, background: 'var(--bg)', border: '1px solid var(--line)', borderRadius: 12, boxShadow: 'var(--shadow)', padding: 12, zIndex: 40 }}>
      <div style={{ display: 'flex', alignItems: 'center', marginBottom: 10 }}>
        <div style={{ fontSize: 12, fontWeight: 600, textTransform: 'uppercase', letterSpacing: '.05em', color: 'var(--faint)' }}>
          Claude accounts
        </div>
        <span style={{ flex: 1 }} />
        <div
          onClick={() => setAdding(!adding)}
          title="Add account — creates Documents/claude-accounts/<name>"
          className="hover-bg"
          style={{ height: 20, padding: '0 8px', borderRadius: 6, background: 'var(--chip)', display: 'flex', alignItems: 'center', gap: 4, fontSize: 12, cursor: 'default' }}
        >
          <span style={{ fontSize: 13, lineHeight: 1 }}>+</span> Add
        </div>
      </div>

      {adding && (
        <div style={{ display: 'flex', gap: 6, marginBottom: 10 }}>
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
            style={{
              flex: 1, height: 29, padding: '0 8px', border: '1px solid var(--line)', borderRadius: 7,
              background: 'var(--panel)', color: 'var(--fg)', font: 'inherit', fontSize: 13, outline: 'none'
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
            style={{ height: 29, padding: '0 10px', borderRadius: 7, background: 'var(--accent)', color: 'oklch(.99 .01 160)', display: 'flex', alignItems: 'center', fontSize: 12.5, fontWeight: 590, cursor: 'default' }}
          >
            Create
          </div>
        </div>
      )}
      {error && <div style={{ fontSize: 12, color: 'oklch(.58 .2 25)', marginBottom: 8 }}>{error}</div>}

      {accounts.length === 0 && (
        <div style={{ fontSize: 12.5, color: 'var(--faint)', padding: '6px 0 2px' }}>
          No accounts yet — add one to spawn sessions with an isolated Claude config.
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
          <div key={acc.name} style={{ padding: '9px 0', borderTop: '1px solid var(--line)' }}>
            <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginBottom: 8 }}>
              <span style={{ width: 7, height: 7, borderRadius: '50%', background: dot }} />
              <span style={{ fontSize: 13.5, fontWeight: 600, whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis' }} title={acc.path}>
                {acc.name}
              </span>
              <span style={{ flex: 1 }} />
              <span style={{ fontSize: 11.5, color: 'var(--faint)', whiteSpace: 'nowrap' }}>
                {note ?? acc.plan}
              </span>
              <span
                onClick={() => setLoginFor(acc)}
                title="Sign in / re-login this account"
                className="hover-bg"
                style={{ height: 18, padding: '0 7px', flex: 'none', borderRadius: 5, display: 'grid', placeItems: 'center', fontSize: 11.5, color: 'var(--dim)', background: 'var(--chip)', cursor: 'default' }}
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
                style={{ width: 18, height: 18, flex: 'none', borderRadius: 5, display: 'grid', placeItems: 'center', fontSize: 12, color: 'var(--faint)', cursor: 'default' }}
              >
                ✕
              </span>
            </div>
            {LK.map(([k, full]) => (
              <div key={k} style={{ display: 'flex', alignItems: 'center', gap: 9, marginBottom: 6 }}>
                <span style={{ fontSize: 12, color: 'var(--dim)', width: 44, whiteSpace: 'nowrap' }}>{full}</span>
                <div style={{ flex: 1, height: 7, borderRadius: 4, background: 'var(--track)', border: '1px solid var(--line)', overflow: 'hidden' }}>
                  <div style={{ height: '100%', borderRadius: 3, width: `${Math.round(acc.limits[k] * 100)}%`, background: limitColor(acc.limits[k]) }} />
                </div>
                <span style={{ fontSize: 12, color: 'var(--dim)', width: 34, textAlign: 'right', fontVariantNumeric: 'tabular-nums' }}>
                  {Math.round(acc.limits[k] * 100)}%
                </span>
                <span style={{ fontSize: 11.5, color: 'var(--faint)', width: 58, textAlign: 'right', whiteSpace: 'nowrap' }}>
                  ↻ {acc.resets[k]}
                </span>
              </div>
            ))}
          </div>
        );
      })}
    </div>
  );
}
