import { useEffect, useState } from 'react';
import {
  setSettingsParent, setSettingsSyncOnStart, settingsSyncState, syncSettingsNow, type SyncStateDto
} from '../../ipc/commands';
import { Icon, ToolButton } from '../../shared/ui/icons';
import { PROVIDER_LABEL, PROVIDERS, type Account, type Provider } from '../../shared/types';

/**
 * One account per provider whose settings file the others copy: chosen here,
 * copied now on a click, or once at every launch with the box ticked. The
 * logins, and Codex's trusted folders, stay each account's own.
 */
export function SettingsSync({ accounts }: { accounts: Account[] }) {
  const [state, setState] = useState<SyncStateDto | null>(null);
  const [note, setNote] = useState<Partial<Record<Provider, string>>>({});
  const [busy, setBusy] = useState<Provider | null>(null);

  useEffect(() => {
    void settingsSyncState().then(setState);
  }, []);

  if (!state) return null;
  const providers = PROVIDERS.filter(p => accounts.filter(a => a.provider === p).length > 1);
  if (!providers.length) return null;

  const syncNow = async (p: Provider) => {
    setBusy(p);
    try {
      const r = await syncSettingsNow(p);
      setNote(n => ({
        ...n,
        [p]: r.failed.length
          ? `Copied to ${r.synced.length}; failed: ${r.failed.join('; ')}`
          : `Copied to ${r.synced.length} account${r.synced.length === 1 ? '' : 's'}`
      }));
    } catch (e) {
      setNote(n => ({ ...n, [p]: String(e) }));
    } finally {
      setBusy(null);
    }
  };

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 6, padding: '8px 10px', marginBottom: 8, borderRadius: 6, background: 'var(--card)', border: '1px solid var(--line)' }}>
      <div style={{ fontSize: 'var(--fs-1)', fontWeight: 700, textTransform: 'uppercase', letterSpacing: '.05em', color: 'var(--faint)' }}>
        Shared settings
      </div>
      {providers.map(p => {
        const parent = state[p];
        return (
          <div key={p} style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
            <Icon name={p} />
            <span style={{ fontSize: 'var(--fs-3)', color: 'var(--dim)', whiteSpace: 'nowrap' }}>{PROVIDER_LABEL[p]} from</span>
            <select
              id={`sync-parent-${p}`}
              value={parent ?? ''}
              onChange={e => void setSettingsParent(p, e.target.value || null).then(setState)}
              style={{ flex: 1, minWidth: 0 }}
              title="The account whose settings file the others copy"
            >
              <option value="">— no parent —</option>
              {accounts.filter(a => a.provider === p).map(a => (
                <option key={a.name} value={a.name}>{a.name}</option>
              ))}
            </select>
            <ToolButton
              icon="sync"
              label="Sync now"
              disabled={!parent || busy === p}
              spin={busy === p}
              title={parent ? `Copy ${parent}'s settings onto the other ${PROVIDER_LABEL[p]} accounts now` : 'Choose a parent first'}
              onClick={() => void syncNow(p)}
            />
          </div>
        );
      })}
      {Object.entries(note).map(([p, text]) => (
        <div key={p} style={{ fontSize: 'var(--fs-2)', color: 'var(--faint)' }}>{PROVIDER_LABEL[p as Provider]}: {text}</div>
      ))}
      <div className="field-row">
        <input
          type="checkbox"
          id="sync-on-start"
          checked={state.onStart}
          onChange={e => void setSettingsSyncOnStart(e.target.checked).then(setState)}
        />
        <label htmlFor="sync-on-start" style={{ cursor: 'default', fontSize: 'var(--fs-3)' }}>
          Copy from the parents every time Luna starts
        </label>
      </div>
      <div style={{ fontSize: 'var(--fs-1)', color: 'var(--faint)', lineHeight: 1.4 }}>
        The parent's keys overwrite, keys only an account has stay. Logins and Codex's trusted
        folders are never copied.
      </div>
    </div>
  );
}
