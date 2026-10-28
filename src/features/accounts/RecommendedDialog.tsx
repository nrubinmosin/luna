import { useEffect, useState } from 'react';
import { createPortal } from 'react-dom';
import { applyRecommended, recommendedSettings, type RecommendationDto } from '../../ipc/commands';
import { useDialogHost } from '../../shared/ui/dialogHost';
import { Icon } from '../../shared/ui/icons';
import { PROVIDER_LABEL, type Account } from '../../shared/types';

const linkStyle = { cursor: 'default', color: 'var(--dim)', textDecoration: 'underline', borderRadius: 2 } as const;

/**
 * Settings worth having for anyone running the CLI inside Luna, offered for
 * one account: what the file says now and what it would say. A tick is what
 * the file will say — the ones already there open ticked, and unticking one
 * takes it back out. Applying touches only the keys that change and leaves
 * the rest of the file — other keys, their order, comments — as it was.
 */
export function RecommendedDialog({ account, onClose, onEdit }: {
  account: Account;
  onClose: () => void;
  /** Opens the same file in the hand editor. */
  onEdit: () => void;
}) {
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
        setPicked(new Set(list.filter(r => r.applied).map(r => r.key)));
      })
      .catch(e => setError(String(e)));

  useEffect(() => {
    void load();
    const onKey = (e: KeyboardEvent) => e.key === 'Escape' && onClose();
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const add = (recs ?? []).filter(r => picked.has(r.key) && !r.applied).map(r => r.key);
  const remove = (recs ?? []).filter(r => !picked.has(r.key) && r.applied).map(r => r.key);

  const apply = async () => {
    setBusy(true);
    setError(null);
    try {
      await applyRecommended(account.provider, account.path, add, remove);
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

  // The everyday set on top of what is there; a matter of taste stays as it is.
  const tickRecommended = () =>
    setPicked(prev => new Set([...prev, ...(recs ?? []).filter(r => r.defaultOn).map(r => r.key)]));

  if (!host) return null;
  const file = account.provider === 'claude' ? 'settings.json' : 'config.toml';
  const changes = [add.length && `+${add.length}`, remove.length && `−${remove.length}`].filter(Boolean).join(' / ');

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
            What works well for {PROVIDER_LABEL[account.provider]} inside Luna, kept in this
            account's <code>{file}</code> unless a key says otherwise. A ticked key is one the file will have; untick one it has to
            take it back out, and {PROVIDER_LABEL[account.provider]} goes back to its own default. The rest
            of the file stays as it is. Sessions already running pick it up when they next start.
          </div>
          <div style={{ display: 'flex', gap: 12, fontSize: 'var(--fs-3)' }}>
            <span className="hover-bg" style={linkStyle} onClick={tickRecommended}>Tick recommended</span>
            <span className="hover-bg" style={linkStyle} onClick={onEdit}>Edit {file}…</span>
          </div>
          <div className="xp-field" style={{ maxHeight: 'min(420px, calc(100vh - 280px))', overflowY: 'auto', padding: 4 }}>
            {!recs && !error && <div style={{ padding: 8, color: 'var(--faint)' }}>Reading the file…</div>}
            {recs?.map(r => {
              const id = `rec-${r.key}`;
              const adding = picked.has(r.key) && !r.applied;
              const removing = !picked.has(r.key) && r.applied;
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
                      {r.applied ? (
                        <>
                          <span style={{ color: 'var(--fg)', fontWeight: 700, textDecoration: removing ? 'line-through' : undefined }}>
                            {r.recommended}
                          </span>
                          {removing ? (
                            <span style={{ color: 'var(--wait)' }}>→ taken out</span>
                          ) : (
                            <span title="The file has it" style={{ display: 'inline-flex' }}><Icon name="check" size={13} /></span>
                          )}
                        </>
                      ) : (
                        <>
                          <span style={{ color: 'var(--faint)' }}>{r.current ?? 'not set'}</span>
                          <span style={{ color: 'var(--faint)' }}>→</span>
                          <span style={{ color: adding ? 'var(--fg)' : 'var(--faint)', fontWeight: 700 }}>{r.recommended}</span>
                          {r.defaultOn && !adding && <span style={{ color: 'var(--faint)', fontFamily: 'inherit' }}>· recommended</span>}
                        </>
                      )}
                    </div>
                    <div style={{ fontSize: 'var(--fs-2)', color: 'var(--dim)' }}>
                      {r.why}
                      {r.file && <> Kept in <code>{r.file}</code>, where {PROVIDER_LABEL[account.provider]} reads it.</>}
                    </div>
                  </div>
                </div>
              );
            })}
          </div>
          {error && <div style={{ fontSize: 'var(--fs-3)', color: 'oklch(.58 .2 25)' }}>{error}</div>}
          <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'flex-end', gap: 8 }}>
            {done && !busy && !changes && <span style={{ flex: 1, fontSize: 'var(--fs-3)', color: 'var(--dim)' }}>Written to {file}.</span>}
            <button onClick={onClose}>Close</button>
            <button className="primary" disabled={busy || !changes} onClick={() => void apply()}>
              {busy ? 'Applying…' : `Apply ${changes}`.trim()}
            </button>
          </div>
        </div>
      </div>
    </div>,
    host
  );
}
