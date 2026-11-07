import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { pickFolder } from '../../ipc/commands';
import { useDialogHost } from '../../shared/ui/dialogHost';
import { Icon, ToolButton } from '../../shared/ui/icons';
import { useChats } from '../chats/chats.store';

const baseName = (path: string) =>
  path
    .replace(/[\\/]+$/, '')
    .split(/[\\/]+/)
    .pop() || path;
const same = (a: string, b: string) =>
  a.replace(/[\\/]+$/, '').toLowerCase() === b.replace(/[\\/]+$/, '').toLowerCase();

const TITLE =
  'Give skills to the sessions Luna starts in a folder of yours — a repository, say — and in its worktrees, or keep\n' +
  'skills from them. It becomes a column here; nothing is written into the folder itself.';

/**
 * The Skills grid's "Add a folder…": a menu of the folders the new-chat
 * dialog lists — where sessions start, so a column on one is sure to reach
 * them — that are not columns yet, and any other folder through the picker,
 * one that holds several repositories, say. Straight to the picker while
 * there is nothing to list.
 */
export function AddFolder({ columns, onAdd }: { columns: string[]; onAdd: (path: string) => void }) {
  const host = useDialogHost();
  const face = useRef<HTMLSpanElement>(null);
  const list = useRef<HTMLDivElement>(null);
  const [open, setOpen] = useState(false);
  const [pos, setPos] = useState<{ left: number; top: number } | null>(null);
  const known = useChats(s => s.folders);
  const offered = known.filter(f => f.picked && !columns.some(c => same(c, f.path)));

  const add = (path: string | null) => {
    setOpen(false);
    if (path && !columns.some(c => same(c, path))) onAdd(path);
  };
  const browse = () => {
    setOpen(false);
    void pickFolder().then(add);
  };

  useLayoutEffect(() => {
    if (!open || !face.current) return;
    const r = face.current.getBoundingClientRect();
    const w = list.current?.offsetWidth ?? 240;
    const h = list.current?.offsetHeight ?? 120;
    const left = Math.min(Math.max(8, r.right - w), window.innerWidth - w - 8);
    const below = r.bottom + 4;
    setPos({ left, top: below + h > window.innerHeight - 8 ? Math.max(8, r.top - h - 4) : below });
  }, [open, offered.length]);

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

  const row = (key: string, label: string, title: string, onClick: () => void) => (
    <div
      key={key}
      role="menuitem"
      className="hover-bg"
      title={title}
      onClick={onClick}
      style={{
        display: 'flex',
        alignItems: 'center',
        gap: 6,
        height: 26,
        padding: '0 8px',
        borderRadius: 4,
        cursor: 'default',
        whiteSpace: 'nowrap'
      }}
    >
      <Icon name="folder" size={14} />
      {label}
    </div>
  );

  return (
    <span ref={face} style={{ display: 'inline-flex' }}>
      <ToolButton
        icon="folder"
        label="Add a folder…"
        title={TITLE}
        active={open}
        onClick={() => (offered.length ? setOpen(o => !o) : browse())}
      />
      {open &&
        host &&
        createPortal(
          <div
            ref={list}
            role="menu"
            className="popover"
            style={{ left: pos?.left ?? -9999, top: pos?.top ?? 0, minWidth: 220, padding: 4 }}
          >
            {offered.map(f => row(f.path, baseName(f.path), f.path, () => add(f.path)))}
            <div style={{ borderTop: '1px solid var(--line)', margin: '4px 0' }} />
            {row(
              'other',
              'Other folder…',
              'Pick any folder — one that holds several repositories gives its skills to every one',
              browse
            )}
          </div>,
          host
        )}
    </span>
  );
}
