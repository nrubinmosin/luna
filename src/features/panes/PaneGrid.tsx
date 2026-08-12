import { useRef } from 'react';
import type { CSSProperties } from 'react';
import { ACCENT } from '../../shared/lib/format';
import { currentSplits, usePanes, type SplitKey } from './panes.store';
import { Pane } from './Pane';

const GAP = 8;

/**
 * Draggable divider between two panes. Sizes are fractions of the parent flex
 * box, so the handle only has to translate a pointer position into 0..1 and
 * hand it to the store — the panes themselves are plain `flex-grow` children.
 */
function Splitter({ axis, split }: { axis: 'x' | 'y'; split: SplitKey }) {
  const ref = useRef<HTMLDivElement>(null);
  const dragging = useRef(false);

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
      dragging.current = false;
      window.removeEventListener('mousemove', move);
      window.removeEventListener('mouseup', up);
      document.body.style.cursor = '';
      document.body.style.userSelect = '';
      ref.current?.style.setProperty('background', 'transparent');
    };
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

export function PaneGrid() {
  const layout = usePanes(s => s.layout);
  const { col, rowL, rowR } = usePanes(currentSplits);

  const pane = (i: number) => <Pane index={i} />;

  const column = (top: number, bottom: number, fraction: number, split: SplitKey) => (
    <div style={box('column')}>
      <div style={slot(fraction)}>{pane(top)}</div>
      <Splitter axis="y" split={split} />
      <div style={slot(1 - fraction)}>{pane(bottom)}</div>
    </div>
  );

  return (
    <div style={{ flex: 1, minHeight: 0, padding: GAP, background: 'var(--panel)', display: 'flex' }}>
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
    </div>
  );
}
