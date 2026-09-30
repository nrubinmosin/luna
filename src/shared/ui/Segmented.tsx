import type { CSSProperties } from 'react';

const segWrap: CSSProperties = { display: 'flex', flexWrap: 'wrap', gap: 2, padding: 2, background: 'var(--chip)', borderRadius: 2 };

/**
 * A row of equal-width choices, one of them pressed. A row that does not fit
 * (Codex lists seven models, each with a long name) wraps onto a second line
 * instead of spilling out of the dialog: every choice is at least as wide as
 * its label, and a line shares out whatever room is left over evenly.
 */
export function Segmented<T extends string>({ items, value, onPick, height = 24 }: {
  items: readonly T[];
  value: T;
  onPick: (v: T) => void;
  height?: number;
}) {
  return (
    <div className="xp-sunken" style={segWrap}>
      {items.map(item => (
        <div
          key={item}
          onClick={() => onPick(item)}
          className={value !== item ? 'hover-bg' : undefined}
          style={{
            flex: '1 1 0', minWidth: 'max-content', padding: '0 6px', height, borderRadius: 2, display: 'grid',
            placeItems: 'center', fontSize: 'var(--fs-4)', fontWeight: 600, cursor: 'default', whiteSpace: 'nowrap',
            background: value === item ? 'var(--bg)' : 'transparent',
            color: value === item ? 'var(--fg)' : 'var(--dim)',
            boxShadow: value === item ? 'var(--border-sunken-outer), var(--border-sunken-inner)' : 'none'
          }}
        >
          {item}
        </div>
      ))}
    </div>
  );
}
