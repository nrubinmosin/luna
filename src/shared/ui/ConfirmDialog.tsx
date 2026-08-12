import { useEffect } from 'react';
import type { ReactNode } from 'react';

/**
 * Modal for actions that cannot be undone. Deliberately not `window.confirm`:
 * that one cannot spell out what else goes with the click.
 */
export function ConfirmDialog({
  title,
  body,
  confirmLabel = 'Delete',
  danger = true,
  onConfirm,
  onCancel
}: {
  title: string;
  body: ReactNode;
  confirmLabel?: string;
  danger?: boolean;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onCancel();
      if (e.key === 'Enter') onConfirm();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [onCancel, onConfirm]);

  return (
    <div
      onClick={e => {
        // The dialog often renders inside a clickable row; don't let a
        // dismissing click also select whatever is behind it.
        e.stopPropagation();
        onCancel();
      }}
      style={{
        position: 'fixed', inset: 0, background: 'oklch(.2 .03 160 / .34)', backdropFilter: 'blur(2px)',
        display: 'grid', placeItems: 'center', zIndex: 80
      }}
    >
      <div
        onClick={e => e.stopPropagation()}
        style={{
          width: 420, background: 'var(--bg)', border: '1px solid var(--line)', borderRadius: 14,
          boxShadow: 'var(--shadow)', overflow: 'hidden'
        }}
      >
        <div style={{ padding: '15px 18px 12px' }}>
          <div style={{ fontSize: 15, fontWeight: 640, marginBottom: 7 }}>{title}</div>
          <div style={{ fontSize: 12.5, lineHeight: 1.5, color: 'var(--dim)' }}>{body}</div>
        </div>
        <div
          style={{
            padding: '11px 18px', borderTop: '1px solid var(--line)', background: 'var(--panel)',
            display: 'flex', justifyContent: 'flex-end', gap: 8
          }}
        >
          <div
            onClick={onCancel}
            className="hover-bg"
            style={{
              height: 31, padding: '0 14px', borderRadius: 8, border: '1px solid var(--line)',
              background: 'var(--bg)', display: 'flex', alignItems: 'center', fontSize: 13.5, cursor: 'default'
            }}
          >
            Cancel
          </div>
          <div
            onClick={onConfirm}
            className="hover-bright"
            style={{
              height: 31, padding: '0 16px', borderRadius: 8, display: 'flex', alignItems: 'center',
              fontSize: 13.5, fontWeight: 590, cursor: 'default', color: 'oklch(.99 .01 160)',
              background: danger ? 'oklch(.55 .2 25)' : 'var(--accent)'
            }}
          >
            {confirmLabel}
          </div>
        </div>
      </div>
    </div>
  );
}
