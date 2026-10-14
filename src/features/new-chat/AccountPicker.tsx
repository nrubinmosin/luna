import { useEffect, useRef, useState } from 'react';
import { accountKey, PROVIDER_LABEL, type Account } from '../../shared/types';
import { Icon } from '../../shared/ui/icons';

/**
 * The account box of the new-chat dialog: a select whose rows carry the
 * provider's mark instead of its name spelled out, which a native <select>
 * cannot draw. Arrows step through the accounts as a select's would; while
 * the list is open, Enter and Escape belong to it, not to the dialog
 * (`data-picker-open` tells the dialog and the keymap to keep off).
 */
export function AccountPicker({ accounts, value, onChange }: {
  accounts: Account[];
  value: string;
  onChange: (key: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const [hot, setHotState] = useState(0);
  // The keyboard handler outlives renders; it reads the row from here.
  const hotRef = useRef(0);
  const setHot = (i: number) => {
    hotRef.current = i;
    setHotState(i);
  };
  const box = useRef<HTMLDivElement>(null);
  const keys = accounts.map(a => accountKey(a.provider, a.name));
  const at = Math.max(0, keys.indexOf(value));
  const current = accounts[at];

  useEffect(() => {
    if (!open) return;
    setHot(at);
    const away = (e: MouseEvent) => {
      if (!box.current?.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape' || e.key === 'Enter') {
        e.preventDefault();
        e.stopPropagation();
        if (e.key === 'Enter') onChange(keys[hotRef.current]);
        setOpen(false);
      } else if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
        e.preventDefault();
        e.stopPropagation();
        setHot((hotRef.current + (e.key === 'ArrowDown' ? 1 : -1) + keys.length) % keys.length);
      }
    };
    document.addEventListener('mousedown', away);
    // Capture, ahead of the dialog's own Enter (which would create the chat).
    window.addEventListener('keydown', onKey, true);
    return () => {
      document.removeEventListener('mousedown', away);
      window.removeEventListener('keydown', onKey, true);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

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
            onChange(keys[(at + (e.key === 'ArrowDown' ? 1 : -1) + keys.length) % keys.length]);
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
          <span className="picker-name" style={{ color: 'var(--faint)' }}>add an account first</span>
        )}
      </button>
      {open && (
        <div className="picker-list" role="listbox">
          {accounts.map((a, i) => (
            <div
              key={keys[i]}
              role="option"
              aria-selected={i === at}
              data-hot={i === hot ? 'yes' : 'no'}
              onMouseEnter={() => setHot(i)}
              onClick={() => {
                onChange(keys[i]);
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
