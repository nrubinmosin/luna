import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import type { Reach } from '../../ipc/commands';
import { useDialogHost } from '../../shared/ui/dialogHost';
import { Icon } from '../../shared/ui/icons';
import { PROVIDER_LABEL, type Provider } from '../../shared/types';

const ALL = 'all';
const PROVIDERS: Provider[] = ['claude', 'codex'];

type AccountRef = { provider: Provider; name: string };

/** What the face says: every account, or by a provider's mark its every account or an account's name. */
function Summary({ reach }: { reach: Reach }) {
  if (!reach.length) return <span className="picker-name" style={{ color: 'var(--faint)' }}>Nowhere</span>;
  if (reach.includes(ALL)) return <span className="picker-name">All accounts</span>;
  return (
    <span className="picker-name" style={{ display: 'flex', alignItems: 'center', gap: 4 }}>
      {reach.map((r, i) => {
        const [provider, name] = r.split('/') as [Provider, string | undefined];
        return (
          <span key={r} style={{ display: 'inline-flex', alignItems: 'center', gap: 3, whiteSpace: 'nowrap' }}>
            <Icon name={provider} size={14} />
            {name ?? `All ${PROVIDER_LABEL[provider]}`}
            {i < reach.length - 1 && ','}
          </span>
        );
      })}
    </span>
  );
}

const describe = (reach: Reach) =>
  !reach.length
    ? 'No account'
    : reach.includes(ALL)
      ? 'Every Claude Code and Codex account'
      : reach
          .map(r => {
            const [provider, name] = r.split('/') as [Provider, string | undefined];
            return name ? `${name} (${PROVIDER_LABEL[provider]})` : `Every ${PROVIDER_LABEL[provider]} account`;
          })
          .join('\n');

/**
 * Which accounts a skill or the instructions go into — every account, every
 * account of one CLI, or accounts picked by name: a face that says which and,
 * under it, a popover of ticks that stays open while they are changed.
 */
export function ReachPicker({ id, reach, accounts, onChange }: {
  id: string;
  reach: Reach;
  accounts: AccountRef[];
  onChange: (reach: Reach) => void;
}) {
  const host = useDialogHost();
  const face = useRef<HTMLButtonElement>(null);
  const list = useRef<HTMLDivElement>(null);
  const [open, setOpen] = useState(false);
  const [pos, setPos] = useState<{ left: number; top: number } | null>(null);

  useLayoutEffect(() => {
    if (!open || !face.current) return;
    const r = face.current.getBoundingClientRect();
    const w = list.current?.offsetWidth ?? 220;
    const h = list.current?.offsetHeight ?? 160;
    const left = Math.min(Math.max(8, r.right - w), window.innerWidth - w - 8);
    const below = r.bottom + 4;
    setPos({ left, top: below + h > window.innerHeight - 8 ? Math.max(8, r.top - h - 4) : below });
  }, [open, reach, accounts]);

  useEffect(() => {
    if (!open) return;
    const away = (e: MouseEvent) => {
      const t = e.target as Node;
      if (!list.current?.contains(t) && !face.current?.contains(t)) setOpen(false);
    };
    // Capture, ahead of the window's own Escape, which would close it too.
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== 'Escape') return;
      e.preventDefault();
      e.stopPropagation();
      setOpen(false);
    };
    document.addEventListener('mousedown', away);
    window.addEventListener('keydown', onKey, true);
    return () => {
      document.removeEventListener('mousedown', away);
      window.removeEventListener('keydown', onKey, true);
    };
  }, [open]);

  const all = reach.includes(ALL);
  const without = (...keys: string[]) => reach.filter(r => !keys.includes(r));
  const providers = PROVIDERS.filter(p => accounts.some(a => a.provider === p) || reach.includes(p));

  const tick = (key: string, label: ReactNode, checked: boolean, disabled: boolean, onTick: (on: boolean) => void, indent = 0) => (
    <div className="field-row" key={key} style={{ margin: 0, paddingLeft: indent, minHeight: 24 }}>
      <input type="checkbox" id={`${id}-${key}`} checked={checked} disabled={disabled} onChange={e => onTick(e.target.checked)} />
      <label htmlFor={`${id}-${key}`} style={{ cursor: 'default', whiteSpace: 'nowrap' }}>
        <span style={{ display: 'inline-flex', alignItems: 'center', gap: 4 }}>{label}</span>
      </label>
    </div>
  );

  return (
    <div className="picker" data-picker-open={open || undefined} style={{ display: 'inline-block', width: 170, verticalAlign: 'middle' }}>
      <button
        ref={face}
        type="button"
        className="picker-face"
        title={describe(reach)}
        onClick={() => setOpen(o => !o)}
      >
        <Summary reach={reach} />
      </button>
      {open &&
        host &&
        createPortal(
          <div ref={list} className="popover" style={{ left: pos?.left ?? -9999, top: pos?.top ?? 0, minWidth: 200, padding: '6px 10px' }}>
            {tick(ALL, 'All accounts', all, false, on => onChange(on ? [ALL] : []))}
            {providers.map(p => {
              const wide = all || reach.includes(p);
              return (
                <div key={p} style={{ borderTop: '1px solid var(--line)', marginTop: 4, paddingTop: 4 }}>
                  {tick(
                    p,
                    <>
                      <Icon name={p} size={14} />
                      Every {PROVIDER_LABEL[p]} account
                    </>,
                    wide,
                    all,
                    on => onChange(on ? [...without(p), p] : without(p))
                  )}
                  {accounts
                    .filter(a => a.provider === p)
                    .map(a => {
                      const key = `${p}/${a.name}`;
                      return tick(key, a.name, wide || reach.includes(key), wide, on => onChange(on ? [...without(key), key] : without(key)), 22);
                    })}
                </div>
              );
            })}
          </div>,
          host
        )}
    </div>
  );
}
