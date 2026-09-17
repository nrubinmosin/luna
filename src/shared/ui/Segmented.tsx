import type { CSSProperties } from 'react';

const segWrap: CSSProperties = { display: 'flex', gap: 2, padding: 2, background: 'var(--chip)', borderRadius: 2 };

/** A row of equal-width choices, one of them pressed. */
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
            flex: 1, height, borderRadius: 2, display: 'grid', placeItems: 'center', fontSize: 'var(--fs-4)',
            fontWeight: 600, cursor: 'default', whiteSpace: 'nowrap',
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
