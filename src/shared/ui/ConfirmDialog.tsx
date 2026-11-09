import { useEffect, useId, useRef } from 'react';
import { createPortal } from 'react-dom';
import type { ReactNode } from 'react';
import { useModal } from './modal';

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
      // Enter confirms, except on a control that Enter works by itself: on a
      // focused Cancel it would otherwise click Cancel and confirm at once.
      const own = e.target instanceof Element && e.target.closest('button, a, summary, select, textarea');
      if (e.key === 'Enter' && !own) onConfirm();
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

  // Modal, over the app or over another dialog (Settings raises the update
  // confirm): everything else is inert until it closes, Tab goes round inside
  // it, and the focus comes back to the control that opened it. It opens on
  // the dialog itself, so Enter confirms and Tab reaches the first control.
  const box = useRef<HTMLDivElement>(null);
  const trap = useModal(box, host);
  const titleId = useId();

  return createPortal(
    <div
      onClick={e => {
        // The dialog often renders inside a clickable row; don't let a
        // dismissing click also select whatever is behind it.
        e.stopPropagation();
        onCancel();
      }}
      className="scrim"
      style={{
        position: 'fixed',
        inset: 0,
        background: 'rgba(0,0,0,.35)',
        backdropFilter: 'blur(1px)',
        display: 'grid',
        placeItems: 'center',
        zIndex: 80
      }}
    >
      <div
        ref={box}
        onClick={e => e.stopPropagation()}
        onKeyDown={trap}
        className="window"
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        tabIndex={-1}
        style={{
          width: 400,
          outline: 'none',
          boxShadow: 'var(--shadow), var(--border-window-outer), var(--border-window-inner)'
        }}
      >
        <div className="title-bar">
          {/* A chat can be named after a whole task, and a title that wraps
              pushes the close button off its own row. */}
          <div className="title-bar-text" id={titleId}>
            {title}
          </div>
          <div className="title-bar-controls">
            <button aria-label="Close" onClick={onCancel} />
          </div>
        </div>
        <div className="window-body">
          <div style={{ fontSize: 'var(--fs-4)', lineHeight: 1.5, color: 'var(--dim)' }}>{body}</div>
          {extra && <div style={{ marginTop: 12 }}>{extra}</div>}
        </div>
        {/* Below the body, which scrolls when the notes or the list run long. */}
        <div className="dialog-foot" style={{ paddingTop: 6 }}>
          <button onClick={onCancel}>Cancel</button>
          <button onClick={onConfirm} className={danger ? 'danger' : 'primary'}>
            {confirmLabel}
          </button>
        </div>
      </div>
    </div>,
    host
  );
}
