import { useState, type ReactNode } from 'react';
import type { LibraryTarget, PlaceDto } from '../../ipc/library';
import { Icon, ToolButton } from '../../shared/ui/icons';
import { PROVIDER_LABEL } from '../../shared/types';
import { GREEN, RED, baseName, sayStatus } from './common';
import { needsLook } from './library.store';

// The small pieces the library's rows and grids are built of.

export const Note = ({ children }: { children: ReactNode }) => (
  <div style={{ fontSize: 'var(--fs-1)', color: 'var(--faint)', marginTop: 4, lineHeight: 1.4 }}>{children}</div>
);

/** Whose a copy is: an account by its CLI's icon and name, a folder's
 *  sessions on an account by the folder's name and then the account's. */
export function TargetLabel({ target }: { target: LibraryTarget }) {
  const account = (
    <>
      <Icon name={target.provider} size={14} />
      {target.name}
    </>
  );
  return target.kind === 'account' ? (
    <span
      style={{ display: 'inline-flex', alignItems: 'center', gap: 4, whiteSpace: 'nowrap' }}
      title={`${PROVIDER_LABEL[target.provider]} account`}
    >
      {account}
    </span>
  ) : (
    <span
      style={{ display: 'inline-flex', alignItems: 'center', gap: 4, whiteSpace: 'nowrap' }}
      title={`Sessions in ${target.path} on ${target.name} (${PROVIDER_LABEL[target.provider]})`}
    >
      <Icon name="folder" size={14} />
      {baseName(target.path)} ·{account}
    </span>
  );
}

/** A copy of the name, to name the skill or instructions in a chat: a tick for a moment once it is on the clipboard. */
export function CopyName({ name }: { name: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <ToolButton
      icon={copied ? 'check' : 'copy'}
      size={20}
      title={copied ? `Copied ${name}` : `Copy ${name}, to name it in a chat`}
      onClick={() =>
        void navigator.clipboard
          .writeText(name)
          .then(() => {
            setCopied(true);
            setTimeout(() => setCopied(false), 1200);
          })
          .catch(() => {})
      }
    />
  );
}

/** A row's name with its copy button; `onDoubleClick` renames, where the row can be. */
export function NameCell({ name, onDoubleClick }: { name: string; onDoubleClick?: () => void }) {
  return (
    <div style={{ display: 'flex', alignItems: 'center', gap: 2 }}>
      <span
        style={{ fontWeight: 700, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}
        title={onDoubleClick ? `${name}\nDouble-click to rename` : name}
        onDoubleClick={onDoubleClick}
      >
        {name}
      </span>
      <CopyName name={name} />
    </div>
  );
}

const DOTS: [string, string][] = [
  [GREEN, 'in place'],
  ['var(--wait)', 'needs a look'],
  [RED, 'cannot be laid out'],
  ['var(--faint)', 'yet to be laid out']
];

/** What a cell's dot says, once above each table that has them (`CellDot`). */
export function DotLegend() {
  return (
    <span
      style={{ display: 'inline-flex', alignItems: 'center', gap: 8, fontSize: 'var(--fs-1)', color: 'var(--faint)' }}
    >
      {DOTS.map(([color, says]) => (
        <span key={says} style={{ display: 'inline-flex', alignItems: 'center', gap: 3, whiteSpace: 'nowrap' }}>
          <span style={{ width: 8, height: 8, borderRadius: '50%', background: color, display: 'inline-block' }} />
          {says}
        </span>
      ))}
    </span>
  );
}

/** A checkbox, or one of a column's radios, with no words beside it: the theme draws the box on a label. */
export function Tick({
  id,
  radio,
  checked,
  mark,
  disabled,
  title,
  onChange
}: {
  id: string;
  /** The group a radio belongs to. */
  radio?: string;
  checked: boolean;
  /** An unticked box drawn as turned off, as ticked for some only, or as ticked (for all or some) by something else (styles/controls.css). */
  mark?: 'off' | 'some' | 'inherited' | 'inherited-some';
  disabled: boolean;
  title: string;
  onChange: (on: boolean) => void;
}) {
  return (
    <>
      <input
        type={radio ? 'radio' : 'checkbox'}
        name={radio}
        id={id}
        checked={checked}
        data-mark={mark}
        disabled={disabled}
        onChange={e => onChange(e.target.checked)}
      />
      <label
        htmlFor={id}
        title={title}
        style={{ display: 'inline-block', width: 0, height: 16, marginLeft: 18, verticalAlign: 'middle' }}
      />
    </>
  );
}

/** A cell's dot: how the copies of one skill in one column stand, worst first. */
export function CellDot({ places }: { places: PlaceDto[] }) {
  if (!places.length) return <span style={{ width: 8, display: 'inline-block' }} />;
  const color = places.some(p => p.status === 'failed')
    ? RED
    : places.some(needsLook)
      ? 'var(--wait)'
      : places.every(p => p.status === 'current' || p.status === 'skipped')
        ? GREEN
        : 'var(--faint)';
  const title = places
    .map(p => `${p.target.name}${p.shownAs ? ` (as ${p.shownAs})` : ''}: ${needsLook(p) ? sayStatus(p) : p.status}`)
    .join('\n');
  return (
    <span
      title={title}
      style={{ width: 8, height: 8, borderRadius: '50%', background: color, display: 'inline-block', marginLeft: 4 }}
    />
  );
}
