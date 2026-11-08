import { useId, useState, type ReactNode } from 'react';
import { Icon } from '../../shared/ui/icons';

/**
 * The pieces every Settings page is built from, so the pages share one rule:
 * a row is a label and a line on what it does on the left and its control on
 * the right rail; a list sits under its row with the list's own actions under
 * it on the left; what a change costs stays in view under the row; the rest
 * of the explanation folds into the group's "How … works".
 */

/** A titled card of rows. `title` is left off where the page is the group. */
export function Group({ title, about, children }: { title?: string; about?: About; children: ReactNode }) {
  return (
    <section className="set-group">
      {title && <h3 className="set-group-title">{title}</h3>}
      <div className="set-card">
        {children}
        {about && <Disclosure summary={about.summary}>{about.body}</Disclosure>}
      </div>
    </section>
  );
}

/** A group's long explanation, folded: its summary names the topic. */
export interface About {
  summary: string;
  body: ReactNode;
}

function Disclosure({ summary, children }: { summary: string; children: ReactNode }) {
  return (
    <details className="set-about">
      <summary>{summary}</summary>
      <div className="set-about-body">{children}</div>
    </details>
  );
}

export interface RowProps {
  label?: ReactNode;
  /** One line on what the setting does. */
  desc?: ReactNode;
  /** What a change costs, kept in view: `warn` marks one that loses work or
   *  widens what an agent may do. */
  note?: ReactNode;
  warn?: boolean;
  /** The control on the right rail. */
  control?: ReactNode;
  /** A list or a field the row owns, under it at full width. */
  below?: ReactNode;
  /** The id of the control the label names. */
  htmlFor?: string;
  descId?: string;
}

export function Row({ label, desc, note, warn, control, below, htmlFor, descId }: RowProps) {
  return (
    <div className="set-row">
      <div className="set-text">
        {label && (
          <label className="set-label" htmlFor={htmlFor}>
            {label}
          </label>
        )}
        {desc && (
          <div className="set-desc" id={descId}>
            {desc}
          </div>
        )}
        {note && (
          <div className={`set-note${warn ? ' set-warn' : ''}`}>
            {warn && <Icon name="warn" size={14} style={{ flex: 'none', marginTop: 1 }} />}
            <span>{note}</span>
          </div>
        )}
      </div>
      {control && <div className="set-rail">{control}</div>}
      {below && <div className="set-below">{below}</div>}
    </div>
  );
}

/** A row whose control is a checkbox. Its box is drawn on a label of its own
 *  (xp.css draws every box on the label after the input); the row's label is
 *  the one that names it. */
export function CheckRow({
  checked,
  disabled,
  onChange,
  below,
  ...row
}: Omit<RowProps, 'control' | 'htmlFor' | 'descId'> & {
  checked: boolean;
  disabled?: boolean;
  onChange: (on: boolean) => void;
}) {
  const id = useId();
  return (
    <Row
      {...row}
      htmlFor={id}
      descId={`${id}-desc`}
      below={below}
      control={
        <span className="set-box">
          <input
            type="checkbox"
            id={id}
            checked={checked}
            disabled={disabled}
            aria-describedby={row.desc ? `${id}-desc` : undefined}
            onChange={e => onChange(e.target.checked)}
          />
          <label htmlFor={id} aria-hidden="true" />
        </span>
      }
    />
  );
}

/** A list box: its rows, a line in place of them while there are none, and
 *  the list's own actions under it on the left. */
export function List({
  label,
  empty,
  actions,
  items,
  extra
}: {
  label: string;
  /** Said in place of the rows while there are none. */
  empty: string;
  actions?: ReactNode;
  items: ReactNode[];
  /** Rows that do not count towards emptiness (the system default browser). */
  extra?: ReactNode;
}) {
  return (
    <>
      <div className="set-list xp-field">
        <div role="list" aria-label={label}>
          {extra}
          {items}
        </div>
        {!items.length && <div className="set-empty">{empty}</div>}
      </div>
      {actions && <div className="set-list-actions">{actions}</div>}
    </>
  );
}

export function ListItem({ children }: { children: ReactNode }) {
  return (
    <div className="set-item" role="listitem">
      {children}
    </div>
  );
}

/** A named command line in a list: cut off at the end until shown in full,
 *  and copied whole either way. */
export function CommandLine({ name, command }: { name: string; command: string }) {
  const [full, setFull] = useState(false);
  const [copied, setCopied] = useState(false);
  return (
    <ListItem>
      <span className="set-item-main">
        <span className="set-item-name">{name}</span>
        {command && (
          <code className={`set-cmd${full ? ' set-cmd-full' : ''}`} title={full ? undefined : command}>
            {command}
          </code>
        )}
      </span>
      <span className="set-item-actions" hidden={!command}>
        <button className="slim" aria-expanded={full} onClick={() => setFull(f => !f)}>
          {full ? 'Less' : 'Show all'}
        </button>
        <button
          className="slim"
          onClick={() =>
            void navigator.clipboard
              .writeText(command)
              .then(() => {
                setCopied(true);
                setTimeout(() => setCopied(false), 1200);
              })
              .catch(() => {})
          }
        >
          {copied ? 'Copied' : 'Copy'}
        </button>
      </span>
    </ListItem>
  );
}
