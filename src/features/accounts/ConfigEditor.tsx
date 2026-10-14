import { useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { readAccountConfig, writeAccountConfig, type ConfigFileDto } from '../../ipc/commands';
import { useDialogHost } from '../../shared/ui/dialogHost';
import { Icon } from '../../shared/ui/icons';
import { PROVIDER_LABEL, type Account } from '../../shared/types';

/** Where a JSON parse breaks, as the line and column the message can name. */
function jsonProblem(text: string): string | null {
  try {
    const v = JSON.parse(text);
    return v && typeof v === 'object' && !Array.isArray(v) ? null : 'settings.json has to be one JSON object: { … }';
  } catch (e) {
    return String(e instanceof Error ? e.message : e);
  }
}

/**
 * The account's own settings file, edited by hand: Claude Code's
 * settings.json, Codex's config.toml. What is typed is what gets written —
 * order, spacing and comments stay the user's — and only once it parses; the
 * Rust side checks again before it touches the file. Ctrl+S saves.
 */
export function ConfigEditor({ account, onClose }: { account: Account; onClose: () => void }) {
  const host = useDialogHost();
  const [file, setFile] = useState<ConfigFileDto | null>(null);
  const [text, setText] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);
  const [saving, setSaving] = useState(false);
  const [leaving, setLeaving] = useState(false);
  const area = useRef<HTMLTextAreaElement>(null);
  const claude = account.provider === 'claude';
  const fileName = claude ? 'settings.json' : 'config.toml';
  const dirty = !!file && text !== file.text;
  // JSON can be checked as it is typed; TOML is checked on save, by the side
  // that has a TOML parser.
  const problem = claude ? jsonProblem(text) : null;

  useEffect(() => {
    void readAccountConfig(account.provider, account.path)
      .then(f => {
        setFile(f);
        setText(f.text);
        requestAnimationFrame(() => area.current?.focus());
      })
      .catch(e => setError(String(e)));
  }, [account.provider, account.path]);

  const save = async () => {
    if (!dirty || saving || problem) return;
    setSaving(true);
    setError(null);
    try {
      await writeAccountConfig(account.provider, account.path, text);
      setFile(f => (f ? { ...f, text, exists: true } : f));
      setSaved(true);
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  };

  // Escape asks once before throwing edits away.
  const close = () => {
    if (dirty && !leaving) {
      setLeaving(true);
      return;
    }
    onClose();
  };

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.preventDefault();
        e.stopPropagation();
        close();
      } else if ((e.ctrlKey || e.metaKey) && e.code === 'KeyS') {
        e.preventDefault();
        e.stopPropagation();
        void save();
      }
    };
    window.addEventListener('keydown', onKey, true);
    return () => window.removeEventListener('keydown', onKey, true);
  });

  const format = () => {
    try {
      setText(JSON.stringify(JSON.parse(text), null, 2) + '\n');
    } catch {
      // the problem line already says why
    }
  };

  if (!host) return null;

  return createPortal(
    <div
      className="scrim"
      onClick={e => {
        e.stopPropagation();
        close();
      }}
      style={{ position: 'fixed', inset: 0, background: 'rgba(0,0,0,.35)', display: 'grid', placeItems: 'center', zIndex: 80 }}
    >
      <div
        onClick={e => e.stopPropagation()}
        className="window"
        style={{
          width: 760, maxWidth: 'calc(100vw - 32px)', height: 'min(640px, calc(100vh - 48px))',
          display: 'flex', flexDirection: 'column',
          boxShadow: 'var(--shadow), var(--border-window-outer), var(--border-window-inner)'
        }}
      >
        <div className="title-bar" style={{ flex: 'none' }}>
          <div className="title-bar-text">{fileName} — {account.name}</div>
          <div className="title-bar-controls">
            <button aria-label="Close" onClick={close} />
          </div>
        </div>
        <div className="window-body" style={{ flex: 1, minHeight: 0, display: 'flex', flexDirection: 'column', gap: 8 }}>
          <div style={{ display: 'flex', alignItems: 'center', gap: 8, fontSize: 'var(--fs-2)', color: 'var(--faint)' }}>
            <Icon name={account.provider} />
            <span style={{ flex: 1, minWidth: 0, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }} title={file?.path}>
              {file ? `${file.path}${file.exists ? '' : ' — not there yet, saving creates it'}` : 'Reading…'}
            </span>
            <span>{PROVIDER_LABEL[account.provider]} reads it when a session starts</span>
          </div>
          <textarea
            ref={area}
            value={text}
            spellCheck={false}
            onChange={e => {
              setText(e.target.value);
              setSaved(false);
              setLeaving(false);
            }}
            onKeyDown={e => {
              // Tab indents rather than leaving the box.
              if (e.key === 'Tab' && !e.ctrlKey && !e.altKey) {
                e.preventDefault();
                const el = e.currentTarget;
                const { selectionStart: a, selectionEnd: b } = el;
                setText(text.slice(0, a) + '  ' + text.slice(b));
                requestAnimationFrame(() => el.setSelectionRange(a + 2, a + 2));
              }
            }}
            className="config-text"
            disabled={!file}
          />
          <div style={{ minHeight: 18, fontSize: 'var(--fs-3)' }}>
            {problem ? (
              <span style={{ color: 'oklch(.58 .2 25)' }}>{problem}</span>
            ) : error ? (
              <span style={{ color: 'oklch(.58 .2 25)' }}>{error}</span>
            ) : leaving ? (
              <span style={{ color: 'var(--wait)' }}>Unsaved changes — Esc again to throw them away</span>
            ) : saved && !dirty ? (
              <span style={{ color: 'var(--dim)', display: 'inline-flex', alignItems: 'center', gap: 6 }}>
                <Icon name="check" size={13} /> Saved. New sessions on this account start with it.
              </span>
            ) : null}
          </div>
          <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
            {claude && (
              <button onClick={format} disabled={!!problem || !file}>
                Format
              </button>
            )}
            <span style={{ flex: 1 }} />
            <button onClick={close}>Close</button>
            <button className="primary" onClick={() => void save()} disabled={!dirty || saving || !!problem}>
              {saving ? 'Saving…' : 'Save'}
            </button>
          </div>
        </div>
      </div>
    </div>,
    host
  );
}
