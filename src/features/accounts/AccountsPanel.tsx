import { useState } from 'react';
import { fmtResetDate } from '../../shared/lib/format';
import { ConfirmDialog } from '../../shared/ui/ConfirmDialog';
import { PROVIDER_LABEL, PROVIDER_VENDOR, PROVIDERS, accountKey, type Account, type Provider } from '../../shared/types';
import { useAccounts } from './accounts.store';
import { useChats } from '../chats/chats.store';
import { SettingsDialog } from '../settings/SettingsDialog';
import { ui } from '../providers';

/** Renders inline as the sidebar's account list — no longer a floating popover. */
export function AccountsPanel() {
  const accounts = useAccounts(s => s.accounts);
  const adding = useAccounts(s => s.adding);
  const error = useAccounts(s => s.error);
  const refreshing = useAccounts(s => s.refreshing);
  const { add, remove, setAdding, setLoginFor, refreshAccount } = useAccounts.getState();
  const [name, setName] = useState('');
  const [provider, setProvider] = useState<Provider>('claude');
  // `?demo=settings` opens the dialog for its screenshot; the check compiles
  // away in a release build along with the rest of the fixture.
  const [settingsOpen, setSettingsOpen] = useState(
    () => import.meta.env.DEV && new URLSearchParams(location.search).get('demo') === 'settings'
  );
  // `window.confirm` is what stood here, and in a webview that is the dialog
  // plugin's `confirm` command — which this app's ACL does not allow. It threw
  // into an unhandled rejection and returned nothing, so the guard read as
  // "cancelled" and an account in use simply could not be deleted.
  const [deleting, setDeleting] = useState<Account | null>(null);
  const folders = useChats(s => s.folders);

  const inUse = (account: Account) =>
    folders.some(f => f.chats.some(c => c.provider === account.provider && c.account === account.name));

  const submit = () => {
    if (!name.trim()) return;
    void add(provider, name);
    setName('');
  };

  return (
    <div style={{ borderTop: '1px solid var(--line)', padding: '7px 9px 9px', overflowY: 'auto' }}>
      <div style={{ display: 'flex', alignItems: 'center', marginBottom: 6 }}>
        <div style={{ fontSize: 'var(--fs-1)', fontWeight: 700, textTransform: 'uppercase', letterSpacing: '.04em', color: 'var(--faint)' }}>
          Accounts
        </div>
        <span style={{ flex: 1 }} />
        <button
          onClick={() => setSettingsOpen(true)}
          title="Settings — accounts folder, versions and updates"
          aria-label="Settings"
          className="slim"
          style={{ minHeight: 'calc(var(--ui) * 1.35)', fontSize: 'var(--fs-2)', marginRight: 4, width: 26 }}
        >
          ⚙
        </button>
        <button
          onClick={() => setAdding(!adding)}
          title="Add account — creates <accounts folder>/anthropic/<name> or /openai/<name>"
          className="slim"
          style={{ minHeight: 'calc(var(--ui) * 1.35)', fontSize: 'var(--fs-2)' }}
        >
          + add
        </button>
      </div>

      {settingsOpen && <SettingsDialog onClose={() => setSettingsOpen(false)} />}

      {adding && (
        <div style={{ display: 'flex', gap: 4, marginBottom: 8 }}>
          <select
            value={provider}
            onChange={e => setProvider(e.target.value as Provider)}
            title="Which CLI this account signs in to"
            style={{ flex: 'none', width: 92 }}
          >
            {PROVIDERS.map(p => (
              <option key={p} value={p}>{PROVIDER_LABEL[p]}</option>
            ))}
          </select>
          <input
            autoFocus
            value={name}
            onChange={e => setName(e.target.value)}
            onKeyDown={e => {
              if (e.key === 'Enter') submit();
              if (e.key === 'Escape') setAdding(false);
            }}
            placeholder="account name"
            type="text"
            style={{ flex: 1, minWidth: 0 }}
          />
          <button onClick={submit} disabled={!name.trim()} className="slim primary">
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
        const p = ui(acc.provider);
        // The dot answers "can this account be used", which is the sign-in
        // state — not whether the usage endpoint happened to answer.
        const dot = !acc.signedIn
          ? 'oklch(.63 .19 25)'
          : acc.sync === 'loading'
            ? 'var(--faint)'
            : p.worstLimit(acc) >= 0.85
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
            ? acc.refreshError
              ? 'token refresh failed'
              : 'waiting for token refresh'
            : acc.sync === 'throttled'
              ? 'usage rate-limited'
              : acc.sync === 'error'
                ? 'usage unavailable'
                : 'no usage data yet'
          : acc.usageAge;
        const weekReset = acc.haveUsage ? fmtResetDate(p.longResetAt(acc)) : null;
        const busy = refreshing.includes(acc.path);
        return (
          <div key={accountKey(acc.provider, acc.name)} style={{ padding: '6px 0', borderTop: '1px solid var(--line)' }}>
            <div style={{ display: 'flex', alignItems: 'center', gap: 6, marginBottom: 5 }}>
              <span style={{ width: 6, height: 6, borderRadius: '50%', flex: 'none', background: dot }} />
              <span
                style={{ fontSize: 'var(--fs-3)', fontWeight: 700, whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis' }}
                title={[acc.email, acc.path].filter(Boolean).join('\n')}
              >
                {acc.name}
              </span>
              <span
                title={`${PROVIDER_LABEL[acc.provider]} — a ${PROVIDER_VENDOR[acc.provider]} subscription`}
                style={{
                  fontSize: 'var(--fs-1)', color: 'var(--dim)', background: 'var(--chip)', borderRadius: 2,
                  padding: '0 4px', flex: 'none', whiteSpace: 'nowrap'
                }}
              >
                {p.label}
              </span>
              <span style={{ flex: 1 }} />
              <span
                title={acc.signedIn ? 'Signed in' : acc.refreshError ?? 'No usable credentials — click login'}
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
                onClick={() => void refreshAccount(acc)}
                title={
                  busy
                    ? 'Refreshing…'
                    : acc.provider === 'claude'
                      ? 'Refresh limits now — renews an expired token on the way'
                      : 'Refresh limits now'
                }
                className="hover-bg"
                style={{
                  width: 16, height: 16, flex: 'none', borderRadius: 2, display: 'grid', placeItems: 'center',
                  fontSize: 'var(--fs-2)', color: 'var(--faint)', cursor: 'default',
                  ...(busy && { animation: 'spin .9s linear infinite', pointerEvents: 'none' })
                }}
              >
                ↻
              </span>
              <span
                onClick={() => setDeleting(acc)}
                title="Delete account folder"
                className="hover-danger"
                style={{ width: 16, height: 16, flex: 'none', borderRadius: 2, display: 'grid', placeItems: 'center', fontSize: 'var(--fs-1)', color: 'var(--faint)', cursor: 'default' }}
              >
                ✕
              </span>
            </div>
            <p.LimitBars account={acc} />
            {(weekReset || usageNote) && (
              <div style={{ display: 'flex', alignItems: 'baseline', gap: 6, fontSize: 'var(--fs-1)', color: 'var(--faint)', marginTop: 1 }}>
                {weekReset && <span style={{ whiteSpace: 'nowrap' }}>resets {weekReset}</span>}
                <span style={{ flex: 1 }} />
                {usageNote && (
                  <span title={acc.refreshError ?? undefined} style={{ textAlign: 'right' }}>
                    {usageNote}
                  </span>
                )}
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
              {inUse(deleting) && (
                <div style={{ marginTop: 8 }}>
                  Chats are still set to this account — they have nowhere to spawn until you
                  point them somewhere else.
                </div>
              )}
            </>
          }
          onCancel={() => setDeleting(null)}
          onConfirm={() => {
            void remove(deleting.provider, deleting.name);
            setDeleting(null);
          }}
        />
      )}
    </div>
  );
}
