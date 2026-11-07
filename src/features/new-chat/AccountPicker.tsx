import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react';
import { accountKey, PROVIDER_LABEL, type Account } from '../../shared/types';
import { Icon } from '../../shared/ui/icons';

/**
 * The account box of the new-chat dialog: a select whose rows carry the
 * provider's mark instead of its name spelled out, which a native <select>
 * cannot draw. Arrows step through the accounts as a select's would; while
 * the list is open, Enter and Escape belong to it, not to the dialog
 * (`data-picker-open` tells the dialog and the keymap to keep off).
 */
export function AccountPicker({
  accounts,
  value,
  onChange
}: {
  accounts: Account[];
  value: string;
  onChange: (key: string) => void;
}) {
  const [open, setOpen] = useState(false);
  // The highlighted row, by account key rather than position: the list can
  // change while open (an account removed, the order shuffled), and the
  // highlight stays on the account it was on, or on nothing once it is gone.
  const [hot, setHotState] = useState<string | null>(null);
  // The keyboard handler outlives renders; it reads the row from here.
  const hotRef = useRef<string | null>(null);
  const setHot = useCallback((key: string | null) => {
    hotRef.current = key;
    setHotState(key);
  }, []);
  const box = useRef<HTMLDivElement>(null);
  const keys = accounts.map(a => accountKey(a.provider, a.name));
  const at = Math.max(0, keys.indexOf(value));
  const current = accounts[at];
  // The list and callback as of the latest render: the handler is set up once
  // per opening, and the list can change under it.
  const live = useRef({ keys, value, onChange });
  useLayoutEffect(() => {
    live.current = { keys, value, onChange };
  });

  useEffect(() => {
    if (!open) return;
    const { keys, value } = live.current;
    setHot(keys[Math.max(0, keys.indexOf(value))] ?? null);
    const away = (e: MouseEvent) => {
      if (!box.current?.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape' || e.key === 'Enter') {
        e.preventDefault();
        e.stopPropagation();
        const { keys, onChange } = live.current;
        const key = hotRef.current;
        if (e.key === 'Enter' && key !== null && keys.includes(key)) onChange(key);
        setOpen(false);
      } else if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
        e.preventDefault();
        e.stopPropagation();
        const { keys } = live.current;
        const down = e.key === 'ArrowDown';
        const from = hotRef.current === null ? -1 : keys.indexOf(hotRef.current);
        // From nothing (or a row gone) a step lands on the first or last row.
        const to = from < 0 ? (down ? 0 : keys.length - 1) : (from + (down ? 1 : -1) + keys.length) % keys.length;
        setHot(keys[to] ?? null);
      }
    };
    document.addEventListener('mousedown', away);
    // Capture, ahead of the dialog's own Enter (which would create the chat).
    window.addEventListener('keydown', onKey, true);
    return () => {
      document.removeEventListener('mousedown', away);
      window.removeEventListener('keydown', onKey, true);
    };
  }, [open, setHot]);

  return (
    <div ref={box} className="picker" data-picker-open={open || undefined}>
      <button
        type="button"
        className="picker-face"
        disabled={!accounts.length}
        onClick={() => setOpen(o => !o)}
        onKeyDown={e => {
          if (open || !keys.length) return;
          if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
            e.preventDefault();
            const key = keys[(at + (e.key === 'ArrowDown' ? 1 : -1) + keys.length) % keys.length];
            if (key !== undefined) onChange(key);
          }
        }}
        title={current ? `${current.name} — ${PROVIDER_LABEL[current.provider]}` : undefined}
      >
        {current ? (
          <>
            <Icon name={current.provider} />
            <span className="picker-name">{current.name}</span>
          </>
        ) : (
          <span className="picker-name" style={{ color: 'var(--faint)' }}>
            add an account first
          </span>
        )}
      </button>
      {open && (
        <div className="picker-list" role="listbox">
          {accounts.map((a, i) => (
            <div
              key={keys[i]}
              role="option"
              aria-selected={i === at}
              data-hot={keys[i] === hot ? 'yes' : 'no'}
              onMouseEnter={() => setHot(keys[i] ?? null)}
              onClick={() => {
                onChange(accountKey(a.provider, a.name));
                setOpen(false);
              }}
              title={PROVIDER_LABEL[a.provider]}
            >
              <Icon name={a.provider} />
              {a.name}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
