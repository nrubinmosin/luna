import { useState } from 'react';
import { fmtResetDate, limitColor } from '../../shared/lib/format';
import { ConfirmDialog } from '../../shared/ui/ConfirmDialog';
import type { Account } from '../../shared/types';
import { useAccounts } from './accounts.store';
import { useChats } from '../chats/chats.store';
import { AccountsRoot } from './AccountsRoot';

const LK: Array<['h5' | 'week' | 'fable', string]> = [['h5', '5 hours'], ['week', 'week'], ['fable', 'fable']];

/** Renders inline as the sidebar's account list — no longer a floating popover. */
export function AccountsPanel() {
  const accounts = useAccounts(s => s.accounts);
  const adding = useAccounts(s => s.adding);
  const error = useAccounts(s => s.error);
  const { add, remove, setAdding, setLoginFor } = useAccounts.getState();
  const [name, setName] = useState('');
  const [rootOpen, setRootOpen] = useState(false);
  // `window.confirm` is what stood here, and in a webview that is the dialog
  // plugin's `confirm` command — which this app's ACL does not allow. It threw
  // into an unhandled rejection and returned nothing, so the guard read as
  // "cancelled" and an account in use simply could not be deleted.
  const [deleting, setDeleting] = useState<Account | null>(null);
  const folders = useChats(s => s.folders);

  const inUse = (account: string) =>
    folders.some(f => f.chats.some(c => c.account === account));

  return (
    <div style={{ borderTop: '1px solid var(--line)', padding: '7px 9px 9px', overflowY: 'auto' }}>
      <div style={{ display: 'flex', alignItems: 'center', marginBottom: 6 }}>
        <div style={{ fontSize: 'var(--fs-1)', fontWeight: 700, textTransform: 'uppercase', letterSpacing: '.04em', color: 'var(--faint)' }}>
          Accounts
        </div>
        <span style={{ flex: 1 }} />
        <button
          onClick={() => setRootOpen(!rootOpen)}
          title="Accounts folder — where the account directories live"
          aria-label="Accounts folder"
          className="slim"
          style={{ minHeight: 'calc(var(--ui) * 1.35)', fontSize: 'var(--fs-2)', marginRight: 4, width: 26 }}
        >
          ⚙
        </button>
        <button
          onClick={() => setAdding(!adding)}
          title="Add account — creates <accounts folder>/<name>"
          className="slim"
          style={{ minHeight: 'calc(var(--ui) * 1.35)', fontSize: 'var(--fs-2)' }}
        >
          + add
        </button>
      </div>

      {rootOpen && <AccountsRoot onClose={() => setRootOpen(false)} />}

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
            type="text"
            style={{ flex: 1, minWidth: 0 }}
          />
          <button
            onClick={() => {
              if (name.trim()) {
                void add(name);
                setName('');
              }
            }}
            disabled={!name.trim()}
            className="slim primary"
          >
            Create
          </button>
        </div>
      )}
      {error && <div style={{ fontSize: 'var(--fs-2)', color: 'oklch(.58 .2 25)', marginBottom: 6 }}>{error}</div>}

      {accounts.length === 0 && (
        <div style={{ fontSize: 'var(--fs-2)', color: 'var(--faint)', padding: '2px 0' }}>
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
        const weekReset = acc.haveUsage ? fmtResetDate(acc.weekResetAt) : null;
        return (
          <div key={acc.name} style={{ padding: '6px 0', borderTop: '1px solid var(--line)' }}>
            <div style={{ display: 'flex', alignItems: 'center', gap: 6, marginBottom: 5 }}>
              <span style={{ width: 6, height: 6, borderRadius: '50%', flex: 'none', background: dot }} />
              <span
                style={{ fontSize: 'var(--fs-3)', fontWeight: 700, whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis' }}
                title={[acc.email, acc.path].filter(Boolean).join('\n')}
              >
                {acc.name}
              </span>
              <span style={{ flex: 1 }} />
              <span
                title={acc.signedIn ? 'Signed in' : 'No usable credentials — click login'}
                style={{
                  fontSize: 'var(--fs-1)', whiteSpace: 'nowrap',
                  color: acc.signedIn ? 'var(--faint)' : 'oklch(.58 .2 25)'
                }}
              >
                {note}
              </span>
              {!acc.signedIn && (
                <button
                  onClick={() => setLoginFor(acc)}
                  title="Sign in this account"
                  className="slim"
                  style={{ flex: 'none', minHeight: 'calc(var(--ui) * 1.25)', fontSize: 'var(--fs-1)' }}
                >
                  login
                </button>
              )}
              <span
                onClick={() => setDeleting(acc)}
                title="Delete account folder"
                className="hover-danger"
                style={{ width: 16, height: 16, flex: 'none', borderRadius: 2, display: 'grid', placeItems: 'center', fontSize: 'var(--fs-1)', color: 'var(--faint)', cursor: 'default' }}
              >
                ✕
              </span>
            </div>
            {LK.map(([k, full]) => (
              <div key={k} style={{ display: 'flex', alignItems: 'center', gap: 5, marginBottom: 3 }}>
                <span style={{ fontSize: 'var(--fs-1)', color: 'var(--dim)', width: 34, flex: 'none', whiteSpace: 'nowrap', overflow: 'hidden' }}>{full}</span>
                <div className="xp-sunken" style={{ flex: 1, height: 6, background: 'var(--track)', overflow: 'hidden' }}>
                  {acc.haveUsage && (
                    <div style={{ height: '100%', width: `${Math.round(acc.limits[k] * 100)}%`, background: limitColor(acc.limits[k]) }} />
                  )}
                </div>
                <span style={{ fontSize: 'var(--fs-1)', color: 'var(--dim)', width: 30, flex: 'none', textAlign: 'right', fontVariantNumeric: 'tabular-nums' }}>
                  {acc.haveUsage ? `${Math.round(acc.limits[k] * 100)}%` : '—'}
                </span>
                <span style={{ fontSize: 'var(--fs-1)', color: 'var(--faint)', width: 42, flex: 'none', textAlign: 'right', whiteSpace: 'nowrap', overflow: 'hidden', fontVariantNumeric: 'tabular-nums' }}>
                  {acc.haveUsage ? acc.resets[k] : ''}
                </span>
              </div>
            ))}
            {(weekReset || usageNote) && (
              <div style={{ display: 'flex', alignItems: 'baseline', gap: 6, fontSize: 'var(--fs-1)', color: 'var(--faint)', marginTop: 1 }}>
                {weekReset && <span style={{ whiteSpace: 'nowrap' }}>resets {weekReset}</span>}
                <span style={{ flex: 1 }} />
                {usageNote && <span style={{ textAlign: 'right' }}>{usageNote}</span>}
              </div>
            )}
          </div>
        );
      })}

      {deleting && (
        <ConfirmDialog
          title="Delete account"
          body={
            <>
              Deletes the folder <b>{deleting.path}</b> and the login stored in it. Nothing
              signs you out anywhere else, and nothing here can bring it back.
              {inUse(deleting.name) && (
                <div style={{ marginTop: 8 }}>
                  Chats are still set to this account — they have nowhere to spawn until you
                  point them somewhere else.
                </div>
              )}
            </>
          }
          onCancel={() => setDeleting(null)}
          onConfirm={() => {
            void remove(deleting.name);
            setDeleting(null);
          }}
        />
      )}
    </div>
  );
}
