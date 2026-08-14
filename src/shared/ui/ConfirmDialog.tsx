import { useEffect } from 'react';
import { createPortal } from 'react-dom';
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

  // Out of the caller's subtree and into the app root. A chat row is
  // `draggable`, and Chromium hands every mouse press inside a draggable
  // subtree to the drag machinery — the dialog rendered there paints fine but
  // takes no hover and no click, checkbox and buttons included. The app root
  // rather than <body> because the theme variables and xp.css overrides all
  // hang off `[data-app]`.
  const host = document.querySelector('[data-app]') ?? document.body;

  return createPortal(
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
        className="window"
        style={{ width: 400, boxShadow: 'var(--shadow), var(--border-window-outer), var(--border-window-inner)' }}
      >
        <div className="title-bar">
          {/* A chat can be named after a whole task, and a title that wraps
              pushes the close button off its own row. */}
          <div className="title-bar-text">{title}</div>
          <div className="title-bar-controls">
            <button aria-label="Close" onClick={onCancel} />
          </div>
        </div>
        <div className="window-body">
          <div style={{ fontSize: 'var(--fs-4)', lineHeight: 1.5, color: 'var(--dim)' }}>{body}</div>
          {extra && <div style={{ marginTop: 12 }}>{extra}</div>}
          <div style={{ marginTop: 16, display: 'flex', justifyContent: 'flex-end', gap: 8 }}>
            <button onClick={onCancel}>Cancel</button>
            <button onClick={onConfirm} className={danger ? 'danger' : 'primary'}>
              {confirmLabel}
            </button>
          </div>
        </div>
      </div>
    </div>,
    host
  );
}
