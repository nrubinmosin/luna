/**
 * A row of equal-width choices, one of them pressed. A row that does not fit
 * (Codex lists seven models, each with a long name) wraps onto a second line
 * instead of spilling out of the dialog: every choice is at least as wide as
 * its label, and a line shares out whatever room is left over evenly.
 * Styled by `.seg` in theme.css.
 */
export function Segmented<T extends string>({ items, value, onPick, height = 24, labels }: {
  items: readonly T[];
  value: T;
  onPick: (v: T) => void;
  height?: number;
  /** What to show for an item, when that is not the value itself. */
  labels?: Partial<Record<T, string>>;
}) {
  return (
    <div className="seg" role="radiogroup">
      {items.map(item => (
        <div
          key={item}
          role="radio"
          aria-checked={value === item}
          tabIndex={value === item ? 0 : -1}
          onClick={() => onPick(item)}
          onKeyDown={e => {
            if (e.key === 'Enter' || e.key === ' ') {
              e.preventDefault();
              onPick(item);
            }
          }}
          style={{ height }}
        >
          {labels?.[item] ?? item}
        </div>
      ))}
    </div>
  );
}
