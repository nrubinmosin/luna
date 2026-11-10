/**
 * The model a chat runs on, as one drop-down: a provider can list a dozen
 * models with long names, which no row of buttons keeps readable. The model a
 * chat gets when nothing is changed carries "· default", so there is never a
 * "default" entry that hides which model it is. Items with a `group` come
 * after the rest, under that group's heading, in the order given.
 */
export function ModelSelect<T extends string>({
  items,
  value,
  onPick,
  defaultValue
}: {
  items: ReadonlyArray<{ value: T; label: string; group?: string }>;
  value: T;
  onPick: (v: T) => void;
  /** The model the files (or the CLI itself) would pick. */
  defaultValue?: T | null;
}) {
  const option = (m: { value: T; label: string }) => (
    <option key={m.value} value={m.value}>
      {m.value === defaultValue ? `${m.label} · default` : m.label}
    </option>
  );
  const groups = [...new Set(items.flatMap(m => (m.group ? [m.group] : [])))];
  return (
    <select value={value} onChange={e => onPick(e.target.value as T)} style={{ width: '100%' }}>
      {items.filter(m => !m.group).map(option)}
      {groups.map(g => (
        <optgroup key={g} label={g}>
          {items.filter(m => m.group === g).map(option)}
        </optgroup>
      ))}
    </select>
  );
}
