/**
 * The model a chat runs on, as one drop-down: a provider can list a dozen
 * models with long names, which no row of buttons keeps readable. The model a
 * chat gets when nothing is changed carries "· default", so there is never a
 * "default" entry that hides which model it is.
 */
export function ModelSelect<T extends string>({ items, value, onPick, defaultValue }: {
  items: ReadonlyArray<{ value: T; label: string }>;
  value: T;
  onPick: (v: T) => void;
  /** The model the files (or the CLI itself) would pick. */
  defaultValue?: T | null;
}) {
  return (
    <select value={value} onChange={e => onPick(e.target.value as T)} style={{ width: '100%' }}>
      {items.map(m => (
        <option key={m.value} value={m.value}>
          {m.value === defaultValue ? `${m.label} · default` : m.label}
        </option>
      ))}
    </select>
  );
}
