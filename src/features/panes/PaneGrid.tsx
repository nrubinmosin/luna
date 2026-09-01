import { useEffect, useRef } from 'react';
import type { CSSProperties, ReactNode } from 'react';
import { ACCENT, tint } from '../../shared/lib/format';
import { chatColorTheme } from '../../shared/ui/chatColors';
import { useChats } from '../chats/chats.store';
import { currentLayout, currentSlots, currentSplits, peeking, usePanes, type SplitKey } from './panes.store';
import { Pane } from './Pane';

const GAP = 8;
/** How far the sheet sits inside the grid, so the board stays visible around it. */
const SHEET_INSET = 22;

/**
 * Draggable divider between two panes. Sizes are fractions of the parent flex
 * box, so the handle only has to translate a pointer position into 0..1 and
 * hand it to the store — the panes themselves are plain `flex-grow` children.
 */
function Splitter({ axis, split }: { axis: 'x' | 'y'; split: SplitKey }) {
  const ref = useRef<HTMLDivElement>(null);
  const dragging = useRef(false);
  // Unmounting mid-drag (switching layout with the mouse down) would otherwise
  // leave the move/up handlers bound to window forever.
  const release = useRef<() => void>(() => {});
  useEffect(() => () => release.current(), []);

  const start = (e: React.MouseEvent) => {
    e.preventDefault();
    const box = ref.current?.parentElement;
    if (!box) return;
    const rect = box.getBoundingClientRect();
    const span = axis === 'x' ? rect.width : rect.height;
    if (span < 1) return;

    dragging.current = true;
    const { setSplit } = usePanes.getState();
    const move = (ev: MouseEvent) => {
      const pos = axis === 'x' ? ev.clientX - rect.left : ev.clientY - rect.top;
      setSplit(split, pos / span);
    };
    const up = () => {
      release.current = () => {};
      dragging.current = false;
      window.removeEventListener('mousemove', move);
      window.removeEventListener('mouseup', up);
      document.body.style.cursor = '';
      document.body.style.userSelect = '';
      ref.current?.style.setProperty('background', 'transparent');
    };
    release.current = up;
    window.addEventListener('mousemove', move);
    window.addEventListener('mouseup', up);
    document.body.style.cursor = axis === 'x' ? 'col-resize' : 'row-resize';
    document.body.style.userSelect = 'none';
    ref.current?.style.setProperty('background', ACCENT);
  };

  const style: CSSProperties = {
    flex: 'none',
    alignSelf: 'stretch',
    borderRadius: 1,
    background: 'transparent',
    transition: 'background .12s',
    cursor: axis === 'x' ? 'col-resize' : 'row-resize',
    ...(axis === 'x'
      ? { width: GAP, marginInline: -1 }
      : { height: GAP, marginBlock: -1 })
  };

  return (
    <div
      ref={ref}
      onMouseDown={start}
      onDoubleClick={() => usePanes.getState().setSplit(split, 0.5)}
      onMouseEnter={e => {
        if (!dragging.current) e.currentTarget.style.background = 'var(--line)';
      }}
      onMouseLeave={e => {
        if (!dragging.current) e.currentTarget.style.background = 'transparent';
      }}
      title="Drag to resize · double-click to even out"
      style={style}
    />
  );
}

const box = (dir: 'row' | 'column'): CSSProperties => ({
  flex: 1,
  minWidth: 0,
  minHeight: 0,
  display: 'flex',
  flexDirection: dir
});

/** A pane sized by fraction: flex-grow carries the ratio, basis 0 keeps it honest. */
const slot = (fraction: number): CSSProperties => ({
  flexGrow: fraction,
  flexBasis: 0,
  minWidth: 0,
  minHeight: 0,
  display: 'flex'
});

const sheetStyle: CSSProperties = {
  position: 'absolute',
  inset: SHEET_INSET,
  zIndex: 50,
  display: 'flex',
  flexDirection: 'column',
  minWidth: 0,
  minHeight: 0,
  borderRadius: 11,
  border: `2px solid ${ACCENT}`,
  background: 'var(--panel)',
  boxShadow: '0 24px 70px rgba(0,0,0,.5)',
  overflow: 'hidden'
};

const inFlowStyle: CSSProperties = { flex: 1, minWidth: 0, minHeight: 0, display: 'flex', flexDirection: 'column' };

/** One numbered pane of the board, as a target to jump to from the peek bar. */
function PaneChip({ index }: { index: number }) {
  const chatId = usePanes(s => currentSlots(s)[index] ?? null);
  const on = usePanes(s => s.peek === index);
  const chat = useChats(s => s.findChat(chatId));
  const swatch = chatColorTheme(chat?.color)?.swatch ?? ACCENT;

  return (
    <span
      onClick={() => usePanes.getState().setPeek(index)}
      title={
        chat
          ? `${chat.name}\nCtrl+${index + 1}`
          : `Pane ${index + 1} is empty\nCtrl+${index + 1}`
      }
      className={on ? undefined : 'hover-bg'}
      style={{
        minWidth: 19, height: 19, padding: '0 4px', flex: 'none', borderRadius: 3,
        display: 'flex', alignItems: 'center', justifyContent: 'center', gap: 4,
        fontSize: 'var(--fs-2)', fontWeight: 700, fontVariantNumeric: 'tabular-nums',
        cursor: 'default', opacity: chat ? 1 : 0.5,
        color: on ? '#fff' : 'var(--dim)',
        background: on ? ACCENT : 'var(--chip)',
        border: `1px solid ${on ? ACCENT : 'var(--line)'}`
      }}
    >
      <span style={{ width: 5, height: 5, borderRadius: 3, flex: 'none', background: on ? '#fff' : swatch }} />
      {index + 1}
    </span>
  );
}

/**
 * The strip that makes a peek read as what it is: a sheet held up over the
 * board for a moment, not a new arrangement. It carries the way out and the
 * numbered way across, so moving between panes at full size is one click and
 * never a return trip through the grid.
 */
function PeekBar({ name }: { name: string | null }) {
  const layout = usePanes(currentLayout);

  return (
    <div
      style={{
        flex: 'none', height: 25, display: 'flex', alignItems: 'center', gap: 8, padding: '0 8px',
        background: tint(18, 'var(--panel)'), borderBottom: `1px solid ${ACCENT}`,
        fontSize: 'var(--fs-2)', color: 'var(--dim)'
      }}
    >
      <span style={{ flex: 'none', fontWeight: 700, letterSpacing: '.05em', textTransform: 'uppercase', color: ACCENT }}>
        Peek
      </span>
      <span style={{ flex: '1 1 auto', minWidth: 0, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
        {name ?? ''}
        <span style={{ color: 'var(--faint)' }}>{name ? ' — ' : ''}the board is still behind, untouched</span>
      </span>
      <span style={{ flex: 'none', display: 'flex', alignItems: 'center', gap: 3 }}>
        {Array.from({ length: layout }, (_, i) => (
          <PaneChip key={i} index={i} />
        ))}
      </span>
      <button
        onClick={() => usePanes.getState().closePeek()}
        title="Back to the grid — Ctrl+0"
        className="slim"
        style={{ flex: 'none', height: 19, padding: '0 8px' }}
      >
        Return
      </button>
    </div>
  );
}

/**
 * Wraps every pane, peeked or not, so that holding one up costs no more than a
 * change of style. Lifting the pane into a different parent would unmount its
 * terminal — an `ensure_session` round trip and a full repaint on every peek
 * and every return — where this leaves the element exactly where it was in the
 * tree and only moves it on screen.
 */
function Sheet({ index, children }: { index: number; children: ReactNode }) {
  const peeked = usePanes(s => s.peek === index);
  const chatId = usePanes(s => currentSlots(s)[index] ?? null);
  const name = useChats(s => s.findChat(chatId)?.name ?? null);

  return (
    <div style={peeked ? sheetStyle : inFlowStyle}>
      {peeked && <PeekBar name={name} />}
      {children}
    </div>
  );
}

/** The same sheet for a chat that has no seat on the board at all. */
function ChatSheet({ chatId }: { chatId: string }) {
  const name = useChats(s => s.findChat(chatId)?.name ?? null);
  return (
    <div style={sheetStyle}>
      <PeekBar name={name} />
      <Pane soloChat={chatId} />
    </div>
  );
}

export function PaneGrid() {
  const layout = usePanes(currentLayout);
  const { col, rowL, rowR } = usePanes(currentSplits);
  const peekChat = usePanes(s => s.peekChat);
  const up = usePanes(peeking);

  const pane = (i: number) => (
    <Sheet index={i}>
      <Pane index={i} />
    </Sheet>
  );

  const column = (top: number, bottom: number, fraction: number, split: SplitKey) => (
    <div style={box('column')}>
      <div style={slot(fraction)}>{pane(top)}</div>
      <Splitter axis="y" split={split} />
      <div style={slot(1 - fraction)}>{pane(bottom)}</div>
    </div>
  );

  return (
    <div style={{ flex: 1, minHeight: 0, padding: GAP, background: 'var(--panel)', display: 'flex', position: 'relative' }}>
      {layout === 1 && <div style={box('row')}>{pane(0)}</div>}

      {layout === 2 && (
        <div style={box('row')}>
          <div style={slot(col)}>{pane(0)}</div>
          <Splitter axis="x" split="col" />
          <div style={slot(1 - col)}>{pane(1)}</div>
        </div>
      )}

      {layout === 3 && (
        <div style={box('row')}>
          <div style={slot(col)}>{pane(0)}</div>
          <Splitter axis="x" split="col" />
          <div style={slot(1 - col)}>{column(1, 2, rowR, 'rowR')}</div>
        </div>
      )}

      {layout === 4 && (
        <div style={box('row')}>
          <div style={slot(col)}>{column(0, 2, rowL, 'rowL')}</div>
          <Splitter axis="x" split="col" />
          <div style={slot(1 - col)}>{column(1, 3, rowR, 'rowR')}</div>
        </div>
      )}

      {/* Dimming the board rather than replacing it is what says "for a
          moment" without a word of explanation, and clicking it is the way
          out that needs no keyboard. */}
      {up && (
        <div
          onClick={() => usePanes.getState().closePeek()}
          title="Back to the grid"
          style={{ position: 'absolute', inset: 0, zIndex: 40, background: 'rgba(0,0,0,.5)', cursor: 'default' }}
        />
      )}
      {peekChat && <ChatSheet chatId={peekChat} />}
    </div>
  );
}
