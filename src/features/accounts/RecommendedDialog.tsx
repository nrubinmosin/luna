import { useEffect, useState } from 'react';
import { createPortal } from 'react-dom';
import { applyRecommended, recommendedSettings, type RecommendationDto } from '../../ipc/commands';
import { useDialogHost } from '../../shared/ui/dialogHost';
import { Icon } from '../../shared/ui/icons';
import { PROVIDER_LABEL, type Account } from '../../shared/types';

/**
 * Settings worth having for anyone running the CLI inside Luna, offered for
 * one account: each one ticked or not, what the file says now and what it
 * would say. Applying writes only the ticked keys and leaves the rest of the
 * file — other keys, their order, comments — as it was.
 */
export function RecommendedDialog({ account, onClose }: { account: Account; onClose: () => void }) {
  const host = useDialogHost();
  const [recs, setRecs] = useState<RecommendationDto[] | null>(null);
  const [picked, setPicked] = useState<Set<string>>(new Set());
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [done, setDone] = useState(false);

  const load = () =>
    recommendedSettings(account.provider, account.path)
      .then(list => {
        setRecs(list);
        setPicked(new Set(list.filter(r => r.defaultOn && !r.applied).map(r => r.key)));
      })
      .catch(e => setError(String(e)));

  useEffect(() => {
    void load();
    const onKey = (e: KeyboardEvent) => e.key === 'Escape' && onClose();
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const apply = async () => {
    setBusy(true);
    setError(null);
    try {
      await applyRecommended(account.provider, account.path, [...picked]);
      setDone(true);
      await load();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const toggle = (key: string) =>
    setPicked(prev => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });

  if (!host) return null;
  const file = account.provider === 'claude' ? 'settings.json' : 'config.toml';

  return createPortal(
    <div
      className="scrim"
      onClick={e => {
        e.stopPropagation();
        onClose();
      }}
      style={{ position: 'fixed', inset: 0, background: 'rgba(0,0,0,.35)', display: 'grid', placeItems: 'center', zIndex: 80 }}
    >
      <div
        onClick={e => e.stopPropagation()}
        className="window"
        style={{ width: 600, maxWidth: 'calc(100vw - 32px)', boxShadow: 'var(--shadow), var(--border-window-outer), var(--border-window-inner)' }}
      >
        <div className="title-bar">
          <div className="title-bar-text">Recommended settings — {account.name}</div>
          <div className="title-bar-controls">
            <button aria-label="Close" onClick={onClose} />
          </div>
        </div>
        <div className="window-body" style={{ display: 'flex', flexDirection: 'column', gap: 10 }}>
          <div style={{ fontSize: 'var(--fs-3)', color: 'var(--dim)', lineHeight: 1.45 }}>
            What works well for {PROVIDER_LABEL[account.provider]} inside Luna, written into this
            account's <code>{file}</code>. Only the ticked keys change; the rest of the file stays as it
            is. Sessions already running pick it up when they next start.
          </div>
          <div className="xp-field" style={{ maxHeight: 'min(420px, calc(100vh - 260px))', overflowY: 'auto', padding: 4 }}>
            {!recs && !error && <div style={{ padding: 8, color: 'var(--faint)' }}>Reading the file…</div>}
            {recs?.map(r => {
              const id = `rec-${r.key}`;
              return (
                <div key={r.key} style={{ display: 'flex', gap: 8, padding: '6px 6px', borderBottom: '1px solid var(--line)' }}>
                  <div className="field-row" style={{ flex: 'none', alignSelf: 'flex-start', paddingTop: 1 }}>
                    <input type="checkbox" id={id} checked={picked.has(r.key)} onChange={() => toggle(r.key)} />
                    <label htmlFor={id} style={{ cursor: 'default', fontFamily: '"JetBrains Mono", monospace', fontSize: 'var(--fs-2)', fontWeight: 700 }}>
                      {r.key}
                    </label>
                  </div>
                  <div style={{ flex: 1, minWidth: 0, display: 'flex', flexDirection: 'column', gap: 2 }}>
                    <div style={{ fontSize: 'var(--fs-2)', fontFamily: '"JetBrains Mono", monospace', display: 'flex', gap: 6, flexWrap: 'wrap', alignItems: 'center' }}>
                      <span style={{ color: 'var(--faint)' }}>{r.current ?? 'not set'}</span>
                      <span style={{ color: 'var(--faint)' }}>→</span>
                      <span style={{ color: 'var(--fg)', fontWeight: 700 }}>{r.recommended}</span>
                      {r.applied && <Icon name="check" size={13} />}
                    </div>
                    <div style={{ fontSize: 'var(--fs-2)', color: 'var(--dim)' }}>{r.why}</div>
                  </div>
                </div>
              );
            })}
          </div>
          {error && <div style={{ fontSize: 'var(--fs-3)', color: 'oklch(.58 .2 25)' }}>{error}</div>}
          <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'flex-end', gap: 8 }}>
            {done && !busy && <span style={{ flex: 1, fontSize: 'var(--fs-3)', color: 'var(--dim)' }}>Written to {file}.</span>}
            <button onClick={onClose}>Close</button>
            <button className="primary" disabled={busy || picked.size === 0} onClick={() => void apply()}>
              {busy ? 'Applying…' : `Apply ${picked.size || ''}`.trim()}
            </button>
          </div>
        </div>
      </div>
    </div>,
    host
  );
}
