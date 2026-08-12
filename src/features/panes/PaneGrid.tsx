import { usePanes } from './panes.store';
import { Pane } from './Pane';

const GRIDS: Record<number, { cols: string; rows: string }> = {
  1: { cols: '1fr', rows: '1fr' },
  2: { cols: '1fr 1fr', rows: '1fr' },
  3: { cols: '1.35fr 1fr', rows: '1fr 1fr' },
  4: { cols: '1fr 1fr', rows: '1fr 1fr' }
};

export function PaneGrid() {
  const layout = usePanes(s => s.layout);
  const grid = GRIDS[layout];

  return (
    <div
      style={{
        flex: 1, minHeight: 0, padding: 10, background: 'var(--panel)', display: 'grid', gap: 10,
        gridTemplateColumns: grid.cols, gridTemplateRows: grid.rows
      }}
    >
      {Array.from({ length: layout }, (_, i) => (
        <Pane key={i} index={i} area={layout === 3 && i === 0 ? '1 / 1 / span 2 / 1' : 'auto'} />
      ))}
    </div>
  );
}
