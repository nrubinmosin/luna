import { useEffect } from 'react';
import { createPortal } from 'react-dom';
import type { ReactNode } from 'react';
import { create } from 'zustand';

/** Raised by F1 (keymap.ts) or the status bar's chip alike. */
export const useShortcuts = create<{ open: boolean; setOpen: (v: boolean) => void }>(set => ({
  open: false,
  setOpen: v => set({ open: v })
}));

type Row = [keys: ReactNode, what: ReactNode];

const KEYBOARD: Row[] = [
  ['Ctrl+N', 'New chat'],
  ['Ctrl+Shift+N', 'Quick chat — no dialog: the folder and account of the chat on screen'],
  ['Ctrl+Shift+1…4', 'Layout: one to four panes'],
  ['Ctrl+1…4', 'Several panes: type into that pane'],
  ['Alt+1…4', 'Several panes: hold that pane up on the sheet, again to put it back'],
  ['Ctrl+1…9', 'One pane: show the chat with that number in the list'],
  ['Ctrl+Tab', 'Next window group, I → II → … and round (Ctrl+Shift+Tab stays with the CLI)'],
  ['Ctrl+0', 'Back to the grid'],
  ['Ctrl+Shift+W', 'The next chat that is waiting for you'],
  ['Ctrl+Shift+B', 'Keep the sidebar open, or let it slide away to the edge'],
  ['Hold Alt', 'Over a chat in the list: a look at it on the sheet until Alt is let go — a click keeps it'],
  ['F1', 'This list'],
  ['Esc', 'Closes a dialog; otherwise left to the CLI, where it interrupts']
];

const MOUSE: Row[] = [
  ['Click a chat', 'Shows it: one pane swaps to it, several put it on the sheet — the same chat again goes back'],
  ['Click outside the sheet', 'Back to the grid — so do the mouse’s Back button and, in the sheet’s bar, the chip of the pane that is up'],
  ['Pointer at the left edge', 'With the sidebar hidden: slides it out over the board until the pointer leaves'],
  ['Drag a chat onto a pane', 'Seats it in that pane'],
  ['Drag a title bar onto a pane', 'Swaps the two panes'],
  ['“Waiting” in the status bar', 'Goes to the next chat that is waiting for you']
];

function Section({ label, rows }: { label: string; rows: Row[] }) {
  return (
    <div style={{ marginBottom: 12 }}>
      <div
        style={{
          fontSize: 'var(--fs-1)', fontWeight: 700, textTransform: 'uppercase',
          letterSpacing: '.04em', color: 'var(--faint)', marginBottom: 5
        }}
      >
        {label}
      </div>
      <div style={{ display: 'grid', gridTemplateColumns: 'auto 1fr', columnGap: 12, rowGap: 5, alignItems: 'baseline' }}>
        {rows.map(([keys, what], i) => (
          <div key={i} style={{ display: 'contents' }}>
            <span
              style={{
                justifySelf: 'start', whiteSpace: 'nowrap', padding: '1px 5px', borderRadius: 2,
                fontSize: 'var(--fs-2)', fontWeight: 600, background: 'var(--chip)',
                border: '1px solid var(--line)', color: 'var(--fg)'
              }}
            >
              {keys}
            </span>
            <span style={{ fontSize: 'var(--fs-3)', color: 'var(--dim)', lineHeight: 1.35 }}>{what}</span>
          </div>
        ))}
      </div>
    </div>
  );
}

/** Every shortcut and mouse gesture in one place. Same host and frame as Settings. */
export function ShortcutsDialog() {
  const open = useShortcuts(s => s.open);
  const close = () => useShortcuts.getState().setOpen(false);

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') useShortcuts.getState().setOpen(false);
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [open]);

  if (!open) return null;
  const host = document.querySelector('[data-app]') ?? document.body;

  return createPortal(
    <div
      onClick={e => {
        e.stopPropagation();
        close();
      }}
      className="scrim"
      style={{
        position: 'fixed', inset: 0, background: 'rgba(0,0,0,.35)', backdropFilter: 'blur(1px)',
        display: 'grid', placeItems: 'center', zIndex: 70
      }}
    >
      <div
        onClick={e => e.stopPropagation()}
        className="window"
        style={{ width: 520, maxWidth: 'calc(100vw - 32px)', boxShadow: 'var(--shadow), var(--border-window-outer), var(--border-window-inner)' }}
      >
        <div className="title-bar">
          <div className="title-bar-text">Shortcuts</div>
          <div className="title-bar-controls">
            <button aria-label="Close" onClick={close} />
          </div>
        </div>
        <div className="window-body" style={{ maxHeight: 'calc(100vh - 120px)', overflowY: 'auto' }}>
          <Section label="Keyboard" rows={KEYBOARD} />
          <Section label="Mouse" rows={MOUSE} />
          <div style={{ fontSize: 'var(--fs-1)', color: 'var(--faint)' }}>
            Luna’s shortcuts work while a terminal has focus; every other key goes on to the CLI untouched.
          </div>
        </div>
      </div>
    </div>,
    host
  );
}
