import { useEffect } from 'react';
import type { ReactNode } from 'react';

/**
 * Modal for actions that cannot be undone. Deliberately not `window.confirm`:
 * that one cannot spell out what else goes with the click.
 */
export function ConfirmDialog({
  title,
  body,
  extra,
  confirmLabel = 'Delete',
  danger = true,
  onConfirm,
  onCancel
}: {
  title: string;
  body: ReactNode;
  /** Controls that change what confirming does, e.g. an opt-in checkbox. */
  extra?: ReactNode;
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
        position: 'fixed', inset: 0, background: 'rgba(0,0,0,.35)', backdropFilter: 'blur(1px)',
        display: 'grid', placeItems: 'center', zIndex: 80
      }}
    >
      <div
        onClick={e => e.stopPropagation()}
        className="xp-raised"
        style={{
          width: 400, background: 'var(--bg)', borderRadius: 3,
          boxShadow: 'var(--shadow), var(--border-raised-outer), var(--border-raised-inner)', overflow: 'hidden'
        }}
      >
        <div className="xp-titlebar" style={{ height: 26, display: 'flex', alignItems: 'center', padding: '0 4px 0 9px', fontSize: 12.5, fontWeight: 700 }}>
          <span style={{ flex: 1 }}>{title}</span>
          <span onClick={onCancel} className="hover-bg" style={{ width: 18, height: 18, borderRadius: 2, display: 'grid', placeItems: 'center', fontSize: 11, color: '#fff', cursor: 'default' }}>✕</span>
        </div>
        <div style={{ padding: '14px 16px 12px' }}>
          <div style={{ fontSize: 12.5, lineHeight: 1.5, color: 'var(--dim)' }}>{body}</div>
          {extra && <div style={{ marginTop: 12 }}>{extra}</div>}
        </div>
        <div
          style={{
            padding: '10px 16px', borderTop: '1px solid var(--line)', background: 'var(--panel)',
            display: 'flex', justifyContent: 'flex-end', gap: 8
          }}
        >
          <div
            onClick={onCancel}
            className="hover-bg xp-raised"
            style={{
              height: 26, padding: '0 13px', borderRadius: 2, border: '1px solid var(--window-frame)',
              background: 'var(--panel)', display: 'flex', alignItems: 'center', fontSize: 12.5, cursor: 'default'
            }}
          >
            Cancel
          </div>
          <div
            onClick={onConfirm}
            className="hover-bright"
            style={{
              height: 26, padding: '0 15px', borderRadius: 2, display: 'flex', alignItems: 'center',
              fontSize: 12.5, fontWeight: 700, cursor: 'default', color: '#fff', border: '1px solid var(--window-frame)',
              background: danger ? '#c0392b' : 'var(--accent)',
              boxShadow: 'inset -1px -1px rgba(0,0,0,.5), inset 1px 1px rgba(255,255,255,.55)'
            }}
          >
            {confirmLabel}
          </div>
        </div>
      </div>
    </div>
  );
}
