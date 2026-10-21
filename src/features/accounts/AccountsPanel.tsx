import { useEffect, useState } from 'react';
import { createPortal } from 'react-dom';
import { create } from 'zustand';
import { Icon, ToolButton } from '../../shared/ui/icons';
import { useDialogHost } from '../../shared/ui/dialogHost';
import { ConfirmDialog } from '../../shared/ui/ConfirmDialog';
import { PROVIDER_LABEL, PROVIDER_VENDOR, PROVIDERS, accountKey, type Account, type Provider } from '../../shared/types';
import { useAccounts } from './accounts.store';
import { useChats } from '../chats/chats.store';
import { useSettings } from '../settings/SettingsDialog';
import { LimitBars, worstLimit } from './limits';
import { RecommendedDialog } from './RecommendedDialog';
import { ConfigEditor } from './ConfigEditor';
import { SettingsSync } from './SettingsSync';
import { useAgentAccounts } from '../agents/agentAccounts.store';
import { recommendedSettings } from '../../ipc/commands';

/** Raised from the status bar: its account summary, or its Accounts button. */
export const useAccountsWindow = create<{ open: boolean; setOpen: (v: boolean) => void }>(set => ({
  open: false,
  setOpen: v => set({ open: v })
}));

/**
 * Every account and everything that can be done to one, in a window of its
 * own. The sidebar used to carry all of it inline — five lines an account,
 * near half the sidebar's height on three accounts — for what is mostly
 * glanced at; the glance now lives in the status bar.
 */
export function AccountsWindow() {
  const open = useAccountsWindow(s => s.open);
  const close = () => useAccountsWindow.getState().setOpen(false);
  const host = useDialogHost();

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      // Dialogs raised from inside (login, delete, settings) close themselves first.
      if (e.key === 'Escape' && !document.querySelector('.scrim ~ .scrim')) close();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [open]);

  if (!open || !host) return null;

  return createPortal(
    <div
      className="scrim"
      onClick={e => {
        e.stopPropagation();
        close();
      }}
      style={{
        position: 'fixed', inset: 0, background: 'rgba(0,0,0,.35)', backdropFilter: 'blur(1px)',
        display: 'grid', placeItems: 'center', zIndex: 65
      }}
    >
      <div
        onClick={e => e.stopPropagation()}
        className="window"
        style={{ width: 640, maxWidth: 'calc(100vw - 32px)', boxShadow: 'var(--shadow), var(--border-window-outer), var(--border-window-inner)' }}
      >
        <div className="title-bar">
          <div className="title-bar-text">Accounts</div>
          <div className="title-bar-controls">
            <button aria-label="Close" onClick={close} />
          </div>
        </div>
        <div className="window-body" style={{ maxHeight: 'calc(100vh - 140px)', overflowY: 'auto' }}>
          <AccountsPanel />
        </div>
      </div>
    </div>,
    host
  );
}

function AccountsPanel() {
  const accounts = useAccounts(s => s.accounts);
  const adding = useAccounts(s => s.adding);
  const error = useAccounts(s => s.error);
  const refreshing = useAccounts(s => s.refreshing);
  const { add, remove, setAdding, setLoginFor, refreshAccount } = useAccounts.getState();
  const [name, setName] = useState('');
  const [provider, setProvider] = useState<Provider>('claude');
  // `window.confirm` is what stood here, and in a webview that is the dialog
  // plugin's `confirm` command — which this app's ACL does not allow. It threw
  // into an unhandled rejection and returned nothing, so the guard read as
  // "cancelled" and an account in use simply could not be deleted.
  const [deleting, setDeleting] = useState<Account | null>(null);
  const [recommending, setRecommending] = useState<Account | null>(null);
  const [editing, setEditing] = useState<Account | null>(null);
  // How many of the recommended settings each account's file has, by key:
  // what the button promises before it is opened. Read again whenever a
  // dialog that can change the file closes.
  const [recCounts, setRecCounts] = useState<Record<string, string>>({});
  const accountPaths = accounts.map(a => `${a.provider}|${a.path}`).join('\n');
  useEffect(() => {
    if (recommending || editing) return;
    let stale = false;
    void Promise.all(
      accounts.map(a =>
        recommendedSettings(a.provider, a.path)
          .then(list => [accountKey(a.provider, a.name), list.length ? `${list.filter(r => r.applied).length}/${list.length}` : ''] as const)
          .catch(() => [accountKey(a.provider, a.name), ''] as const)
      )
    ).then(pairs => !stale && setRecCounts(Object.fromEntries(pairs)));
    return () => {
      stale = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [accountPaths, recommending, editing]);
  // The account whose name is being typed, by key, and what it says so far.
  const [renaming, setRenaming] = useState<{ key: string; to: string; error: string | null } | null>(null);
  const commitRename = async (acc: Account) => {
    const to = renaming?.to.trim() ?? '';
    if (!renaming || !to || to === acc.name) {
      setRenaming(null);
      return;
    }
    try {
      await useAccounts.getState().rename(acc.provider, acc.name, to);
      setRenaming(null);
    } catch (e) {
      setRenaming(r => (r ? { ...r, error: String(e) } : r));
    }
  };
  const folders = useChats(s => s.folders);
  const blocked = useAgentAccounts(s => s.blocked);
  const agentsMay = (p: Provider, n: string) => !blocked.includes(`${p}/${n}`);
  const setAgentAllowed = useAgentAccounts.getState().setAllowed;

  const inUse = (account: Account) =>
    folders.some(f => f.chats.some(c => c.provider === account.provider && c.account === account.name));

  const submit = () => {
    if (!name.trim()) return;
    void add(provider, name);
    setName('');
  };

  return (
    <div>
      <div style={{ display: 'flex', alignItems: 'center', gap: 4, marginBottom: 8 }}>
        <ToolButton
          icon="plus"
          label="Add account"
          active={adding}
          title="Add account — creates <accounts folder>/anthropic/<name> or /openai/<name>"
          onClick={() => setAdding(!adding)}
        />
        <span style={{ flex: 1 }} />
        <ToolButton
          icon="gear"
          label="Settings"
          title="Settings — accounts folder, new chats, versions and updates"
          onClick={() => useSettings.getState().setOpen(true)}
        />
      </div>


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

      <SettingsSync accounts={accounts} />

      {accounts.map(acc => {
        // The dot answers "can this account be used", which is the sign-in
        // state — not whether the usage endpoint happened to answer.
        const dot = !acc.signedIn
          ? 'oklch(.63 .19 25)'
          : acc.sync === 'loading'
            ? 'var(--faint)'
            : worstLimit(acc) >= 0.85
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
        const busy = refreshing.includes(acc.path);
        return (
          <div key={accountKey(acc.provider, acc.name)} style={{ padding: '9px 0 8px', borderTop: '1px solid var(--line)', animation: 'pop-in .2s ease-out' }}>
            <div style={{ display: 'flex', alignItems: 'center', gap: 6, marginBottom: 6 }}>
              <span style={{ width: 8, height: 8, borderRadius: '50%', flex: 'none', background: dot }} />
              <span title={`${PROVIDER_LABEL[acc.provider]} — a ${PROVIDER_VENDOR[acc.provider]} subscription`} style={{ flex: 'none', display: 'flex' }}>
                <Icon name={acc.provider} />
              </span>
              {renaming?.key === accountKey(acc.provider, acc.name) ? (
                <input
                  autoFocus
                  type="text"
                  value={renaming.to}
                  onChange={e => setRenaming({ ...renaming, to: e.target.value, error: null })}
                  onKeyDown={e => {
                    if (e.key === 'Enter') void commitRename(acc);
                    if (e.key === 'Escape') {
                      e.stopPropagation();
                      setRenaming(null);
                    }
                  }}
                  onBlur={() => void commitRename(acc)}
                  style={{ width: 180, height: 'calc(var(--ui) * 1.7)', fontWeight: 700 }}
                />
              ) : (
                <span
                  onDoubleClick={() => setRenaming({ key: accountKey(acc.provider, acc.name), to: acc.name, error: null })}
                  style={{ fontSize: 'var(--fs-5)', fontWeight: 700, whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis' }}
                  title={[acc.email, acc.path].filter(Boolean).join('\n')}
                >
                  {acc.name}
                </span>
              )}
              <ToolButton
                icon="pencil"
                size={22}
                title="Rename the account — its folder, and every chat on it follows"
                onClick={() => setRenaming({ key: accountKey(acc.provider, acc.name), to: acc.name, error: null })}
              />
              <span style={{ flex: 1 }} />
              <span
                title={acc.signedIn ? 'Signed in' : acc.refreshError ?? 'No usable credentials — sign in'}
                style={{
                  fontSize: 'var(--fs-3)', whiteSpace: 'nowrap', marginRight: 4,
                  color: acc.signedIn ? 'var(--faint)' : 'oklch(.58 .2 25)'
                }}
              >
                {note}
              </span>
              {!acc.signedIn && (
                <ToolButton icon="key" size={24} label="Sign in" title="Sign in this account" onClick={() => setLoginFor(acc)} />
              )}
              <ToolButton
                icon="recommend"
                size={24}
                label={recCounts[accountKey(acc.provider, acc.name)] || undefined}
                title={
                  recCounts[accountKey(acc.provider, acc.name)]
                    ? `Recommended settings for this account — ${recCounts[accountKey(acc.provider, acc.name)]} in its file`
                    : 'Recommended settings for this account'
                }
                onClick={() => setRecommending(acc)}
              />
              <ToolButton
                icon="file"
                size={24}
                title={`Edit ${acc.provider === 'claude' ? 'settings.json' : 'config.toml'} by hand`}
                onClick={() => setEditing(acc)}
              />
              <ToolButton
                icon="agents"
                size={24}
                active={agentsMay(acc.provider, acc.name)}
                title={
                  agentsMay(acc.provider, acc.name)
                    ? 'Agents may spawn sessions on this account — click to forbid'
                    : 'Agents may not use this account — click to allow'
                }
                style={agentsMay(acc.provider, acc.name) ? undefined : { opacity: 0.55 }}
                onClick={() => void setAgentAllowed(acc.provider, acc.name, !agentsMay(acc.provider, acc.name))}
              />
              <ToolButton
                icon="refresh"
                size={24}
                spin={busy}
                disabled={busy}
                title={
                  busy
                    ? 'Refreshing…'
                    : acc.provider === 'claude'
                      ? 'Refresh limits now — renews an expired token on the way'
                      : 'Refresh limits now'
                }
                onClick={() => void refreshAccount(acc)}
              />
              <ToolButton icon="trash" size={24} danger title="Delete account folder — asks first" onClick={() => setDeleting(acc)} />
            </div>
            {renaming?.key === accountKey(acc.provider, acc.name) && renaming.error && (
              <div style={{ fontSize: 'var(--fs-2)', color: 'oklch(.58 .2 25)', marginBottom: 6 }}>{renaming.error}</div>
            )}
            <LimitBars account={acc} />
            {usageNote && (
              <div title={acc.refreshError ?? undefined} style={{ fontSize: 'var(--fs-1)', color: 'var(--faint)', marginTop: 4, textAlign: 'right' }}>
                {usageNote}
              </div>
            )}
          </div>
        );
      })}

      {recommending && (
        <RecommendedDialog
          account={recommending}
          onClose={() => setRecommending(null)}
          onEdit={() => {
            setEditing(recommending);
            setRecommending(null);
          }}
        />
      )}
      {editing && <ConfigEditor account={editing} onClose={() => setEditing(null)} />}

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
