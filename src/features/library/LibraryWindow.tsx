import { useEffect, useRef, useState, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import {
  libraryDeleteInstructions, libraryDeleteSkill, libraryNewInstructions, libraryNewSkill, libraryOpen,
  libraryRenameInstructions, libraryResolve, librarySetAccounts, librarySetFolder, librarySetFolderSkill,
  librarySetFolderInstructions, librarySetInstructions, librarySetSource, librarySkip, librarySync, pickFolder,
  type LibraryEntry, type LibraryFolderDto, type LibraryInstructionsDto, type LibraryOverviewDto, type LibraryTarget, type PlaceDto, type Reach
} from '../../ipc/commands';
import { useDialogHost } from '../../shared/ui/dialogHost';
import { ConfirmDialog } from '../../shared/ui/ConfirmDialog';
import { Icon, ToolButton } from '../../shared/ui/icons';
import { PROVIDERS, PROVIDER_LABEL, type Provider } from '../../shared/types';
import { AddFolder } from './AddFolder';
import { DiffView } from './DiffView';
import { LibraryEditor } from './LibraryEditor';
import { needsLook, useLibrary, waiting } from './library.store';
import { ReachPicker } from './ReachPicker';

const RED = 'oklch(.58 .2 25)';
const GREEN = 'oklch(.64 .18 145)';

const baseName = (path: string) => path.replace(/[\\/]+$/, '').split(/[\\/]/).pop() || path;

function Section({ label, aside, children }: { label: string; aside?: ReactNode; children: ReactNode }) {
  return (
    <div style={{ marginBottom: 14 }}>
      <div style={{ display: 'flex', alignItems: 'center', gap: 6, marginBottom: 6 }}>
        <span style={{ fontSize: 'var(--fs-1)', fontWeight: 700, textTransform: 'uppercase', letterSpacing: '.04em', color: 'var(--faint)' }}>
          {label}
        </span>
        <span style={{ flex: 1 }} />
        {aside}
      </div>
      {children}
    </div>
  );
}

const Note = ({ children }: { children: ReactNode }) => (
  <div style={{ fontSize: 'var(--fs-1)', color: 'var(--faint)', marginTop: 4, lineHeight: 1.4 }}>{children}</div>
);

/** Whose a copy is: an account by its CLI's icon and name, a folder's
 *  sessions on an account by the folder's name and then the account's. */
function TargetLabel({ target }: { target: LibraryTarget }) {
  const account = (
    <>
      <Icon name={target.provider} size={14} />
      {target.name}
    </>
  );
  return target.kind === 'account' ? (
    <span style={{ display: 'inline-flex', alignItems: 'center', gap: 4, whiteSpace: 'nowrap' }} title={`${PROVIDER_LABEL[target.provider]} account`}>
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
function CopyName({ name }: { name: string }) {
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
function NameCell({ name, onDoubleClick }: { name: string; onDoubleClick?: () => void }) {
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

/** The window's width with one folder, and what each further folder's column adds: the names keep their room. */
const WINDOW_WIDTH = 760;
const FOLDER_WIDTH = 130;

const DOTS: [string, string][] = [[GREEN, 'in place'], ['var(--wait)', 'needs a look'], [RED, 'cannot be laid out'], ['var(--faint)', 'yet to be laid out']];

/** What a cell's dot says, once above each table that has them (`CellDot`). */
function DotLegend() {
  return (
    <span style={{ display: 'inline-flex', alignItems: 'center', gap: 8, fontSize: 'var(--fs-1)', color: 'var(--faint)' }}>
      {DOTS.map(([color, says]) => (
        <span key={says} style={{ display: 'inline-flex', alignItems: 'center', gap: 3, whiteSpace: 'nowrap' }}>
          <span style={{ width: 8, height: 8, borderRadius: '50%', background: color, display: 'inline-block' }} />
          {says}
        </span>
      ))}
    </span>
  );
}

const what = (p: PlaceDto) =>p.skill ?? (p.instructions ? `${p.instructions} instructions` : 'Instructions');

type AccountRef = { provider: Provider; name: string };
const accountKey = (a: AccountRef) => `${a.provider}/${a.name}`;

/** How a reach takes in an account, as library.rs `closeness` ranks it: the closest wins it. */
function how(reach: Reach, a: AccountRef): string | null {
  if (reach.includes(accountKey(a))) return 'named for it';
  if (reach.includes(a.provider)) return `as every ${PROVIDER_LABEL[a.provider]} account`;
  if (reach.includes('all')) return 'as every account';
  return null;
}

function sayStatus(p: PlaceDto): string {
  switch (p.status) {
    case 'changed':
      return p.both ? 'changed there, and in the library too, since Luna wrote it' : 'changed there since Luna wrote it';
    case 'gone':
      return 'deleted there';
    case 'taken':
      return 'something Luna did not write is in its place';
    case 'failed':
      return p.problem ?? 'cannot be laid out';
    default:
      return p.status;
  }
}

/** One copy that waits on the user, with what can be done about it; `count`
 *  copies when they fail for one reason, which is fixed in the library. */
function LookRow({ place, count, act }: { place: PlaceDto; count: number; act: (p: Promise<LibraryOverviewDto>) => void }) {
  const [showDiff, setShowDiff] = useState(false);
  const differs = place.status === 'changed' || place.status === 'taken';
  return (
    <div style={{ borderTop: '1px solid var(--line)', padding: '6px 0' }}>
      <div style={{ display: 'flex', alignItems: 'center', gap: 8, flexWrap: 'wrap' }}>
        {count > 1 ? <span style={{ whiteSpace: 'nowrap' }}>{count} places</span> : <TargetLabel target={place.target} />}
        <b>{what(place)}</b>
        <span
          style={{ flex: 1, minWidth: 160, fontSize: 'var(--fs-2)', color: place.status === 'failed' ? RED : 'var(--dim)' }}
          title={place.dest}
        >
          {sayStatus(place)}
        </span>
        {differs && (
          <button className="slim" onClick={() => setShowDiff(!showDiff)}>
            {showDiff ? 'Hide differences' : 'Differences'}
          </button>
        )}
        {differs && (
          <button className="slim" title="This copy becomes the library's, and so every other copy's" onClick={() => act(libraryResolve(place.dest, true))}>
            Take into library
          </button>
        )}
        {(differs || place.status === 'gone') && (
          <button
            className="slim"
            title="The library's copy goes there; what is there now is backed up first"
            onClick={() => act(libraryResolve(place.dest, false))}
          >
            {place.status === 'gone' ? 'Put back' : place.status === 'taken' ? 'Replace' : 'Put library’s back'}
          </button>
        )}
        {count === 1 && (
          <button className="slim" title="Stop laying it out there; whatever is there stays as it is" onClick={() => act(librarySkip(place.dest, true))}>
            Leave out
          </button>
        )}
      </div>
      {showDiff && <DiffView dest={place.dest} />}
    </div>
  );
}

/** A checkbox, or one of a column's radios, with no words beside it: the theme draws the box on a label. */
function Tick({ id, radio, checked, mark, disabled, title, onChange }: {
  id: string;
  /** The group a radio belongs to. */
  radio?: string;
  checked: boolean;
  /** An unticked box drawn as turned off, as ticked for some only, or as ticked (for all or some) by something else (theme.css). */
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
      <label htmlFor={id} title={title} style={{ display: 'inline-block', width: 0, height: 16, marginLeft: 18, verticalAlign: 'middle' }} />
    </>
  );
}

/**
 * Which accounts' sessions in a folder get a skill: by a reach of the
 * folder's own — a tick where every account gets it there, a dash where some
 * do, a cross where none does — or, as the folder says nothing of it, by what
 * they get anyway, drawn faint. A click opens the accounts to pick from.
 */
function FolderCell({ id, skill, folder, accounts, act }: {
  id: string;
  skill: string;
  folder: LibraryFolderDto;
  accounts: AccountRef[];
  act: (p: Promise<LibraryOverviewDto>) => Promise<boolean>;
}) {
  // Own keys only: a skill may be named `constructor`.
  const own = Object.hasOwn(folder.skills, skill) ? folder.skills[skill] : null;
  const inherited = Object.hasOwn(folder.inherited, skill) ? folder.inherited[skill] : [];
  const gets = own ? accounts.filter(a => how(own, a)).map(accountKey) : inherited;
  const every = accounts.length > 0 && gets.length === accounts.length;
  const mark = own
    ? every
      ? undefined
      : gets.length
        ? 'some'
        : 'off'
    : gets.length
      ? every
        ? 'inherited'
        : 'inherited-some'
      : undefined;
  const named = (key: string) => {
    const [provider, name] = key.split('/') as [Provider, string];
    return `${name} (${PROVIDER_LABEL[provider]})`;
  };
  const who = every ? 'every account' : gets.length ? gets.map(named).join(', ') : 'no account';
  const why = own ? 'picked here' : 'as outside this folder';
  return (
    <ReachPicker
      id={id}
      reach={own}
      accounts={accounts}
      inherit={{ label: 'As outside this folder', gets: inherited }}
      onChange={r => act(librarySetFolderSkill(folder.path, skill, r))}
      face={open => (
        <Tick
          id={id}
          title={`Sessions in ${folder.path} get ${skill} on ${who}, ${why}\nClick to pick the accounts`}
          checked={!!own && every}
          mark={mark}
          disabled={false}
          // Opens the accounts rather than ticking: the box stays as the reach has it.
          onChange={open}
        />
      )}
    />
  );
}

/** A cell's dot: how the copies of one skill in one column stand, worst first. */
function CellDot({ places }: { places: PlaceDto[] }) {
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
  return <span title={title} style={{ width: 8, height: 8, borderRadius: '50%', background: color, display: 'inline-block', marginLeft: 4 }} />;
}

/** A select's value for none at all, which no instructions' name can be (library.rs `check_name`). */
const NO_INSTRUCTIONS = ' none';

/** A folder's entry that decides for an account, as library.rs `folder_instructions` picks it — its own, its
 *  CLI's, all — with its key. */
function folderEntry(instructions: Record<string, string | null> | undefined, a: AccountRef): { key: string; choice: string | null } | undefined {
  for (const key of [accountKey(a), a.provider, 'all']) if (instructions && Object.hasOwn(instructions, key)) return { key, choice: instructions[key] };
  return undefined;
}

/**
 * What the sessions on an account in a folder get in place of its
 * instructions: what holds outside the folder, none, or other instructions.
 * Codex reads its account's AGENTS.md in every session and has no way to
 * leave it out of one, so its accounts' boxes stay as they are.
 */
function FolderInstructions({ folder, account, names, act }: {
  folder: LibraryFolderDto;
  account: AccountRef;
  names: string[];
  act: (p: Promise<LibraryOverviewDto | null>) => Promise<boolean>;
}) {
  const decides = folderEntry(folder.instructions, account);
  const entry = decides?.choice;
  const value = decides === undefined ? '' : (entry ?? NO_INSTRUCTIONS);
  const codex = account.provider === 'codex';
  // An entry for every account, or every one of a CLI, comes only of settings
  // edited by hand: a pick here names the account, and can override it but
  // not take it back.
  const wide =
    decides && decides.key !== accountKey(account)
      ? `\nSet ${decides.key === 'all' ? 'for every account' : `for every ${PROVIDER_LABEL[account.provider]} account`} in the settings file: picking here sets ${account.name}'s own, which "As outside" takes back to that`
      : '';
  const title = codex
    ? `Codex reads ${account.name}'s AGENTS.md in every session and has no way to leave it out of one: a folder cannot change its instructions`
    : `Sessions on ${account.name} in ${folder.path}: ${
        value === ''
          ? 'the instructions they get outside it'
          : value === NO_INSTRUCTIONS
            ? 'no instructions — the account’s file is left out of them'
            : `${value} in place of the account’s`
      }${wide}`;
  return (
    <select
      value={value}
      disabled={codex}
      title={title}
      style={{ maxWidth: 104, color: value === '' ? 'var(--faint)' : undefined }}
      onChange={e => {
        const v = e.target.value;
        void act(librarySetFolderInstructions(folder.path, accountKey(account), v === '', v === '' || v === NO_INSTRUCTIONS ? null : v));
      }}
    >
      <option value="">As outside</option>
      <option value={NO_INSTRUCTIONS}>None</option>
      {[...names, ...(entry && !names.includes(entry) ? [entry] : [])].map(n => (
        <option key={n} value={n}>
          {n}
        </option>
      ))}
    </select>
  );
}

/**
 * The library's instructions against the accounts: a row each, and a column
 * per account whose radios say which one it gets — one at most, or none. A
 * radio names the account in that row's reach, which takes it from whichever
 * other row named it; the reach picker keeps the wider ways in, every account
 * or every one of a CLI, which a closer reach of another row beats.
 */
function InstructionsGrid({ o, busy, act, edit, remove }: {
  o: LibraryOverviewDto;
  busy: boolean;
  act: (p: Promise<LibraryOverviewDto | null>) => Promise<boolean>;
  edit: (name: string) => void;
  remove: (name: string) => void;
}) {
  const [renaming, setRenaming] = useState<{ from: string; to: string } | null>(null);
  const commitRename = () => {
    const to = renaming?.to.trim() ?? '';
    setRenaming(null);
    if (renaming && to && to !== renaming.from) void act(libraryRenameInstructions(renaming.from, to));
  };
  // The accounts by CLI, so a CLI's columns stand under one heading and two
  // accounts of one name, one in each CLI, are told apart by it.
  const cols = PROVIDERS.flatMap(provider => o.accounts.filter(a => a.provider === provider));
  const copies = (i: string, a: AccountRef) =>
    o.places.filter(p => p.instructions === i && p.target.kind === 'account' && p.target.provider === a.provider && p.target.name === a.name);

  const row = (i: LibraryInstructionsDto) => (
    <tr key={i.name}>
      <td style={{ maxWidth: 0, width: '100%' }}>
        {renaming?.from === i.name ? (
          <input
            autoFocus
            type="text"
            value={renaming.to}
            onChange={e => setRenaming({ ...renaming, to: e.target.value })}
            onKeyDown={e => {
              // Through the blur, so the rename is sent once, not by both.
              if (e.key === 'Enter') e.currentTarget.blur();
              if (e.key === 'Escape') {
                e.stopPropagation();
                setRenaming(null);
              }
            }}
            onBlur={commitRename}
            style={{ width: '100%', fontWeight: 700 }}
          />
        ) : (
          <NameCell name={i.name} onDoubleClick={() => setRenaming({ from: i.name, to: i.name })} />
        )}
        <div
          title={i.problem ?? i.description}
          style={{ fontSize: 'var(--fs-2)', color: i.problem ? 'var(--wait)' : 'var(--faint)', overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}
        >
          {i.problem ?? i.description}
        </div>
      </td>
      <td className="cell">
        <ReachPicker id={`lib-ins-${i.name}`} reach={i.accounts} accounts={o.accounts} onChange={r => act(librarySetInstructions(i.name, r ?? []))} />
      </td>
      {cols.map(a => {
        const key = accountKey(a);
        const gets = o.instructionsFor[key] === i.name;
        const way = how(i.accounts, a);
        return (
          <td key={key} className="cell">
            <Tick
              id={`lib-ins-${i.name}-${key}`}
              radio={`lib-ins-${key}`}
              checked={gets}
              // A radio sends the row's whole reach with the account added: one
              // made before the last answer is in would build on a stale reach.
              disabled={busy}
              title={
                gets
                  ? `${a.name} gets ${i.name}, ${way}`
                  : way
                    ? `${i.name} takes in ${a.name} ${way}, but another one names it more closely\nClick to give it ${i.name}`
                    : `Give ${a.name} ${i.name}`
              }
              onChange={() => void act(librarySetInstructions(i.name, [...i.accounts, key]))}
            />
            <CellDot places={copies(i.name, a)} />
          </td>
        );
      })}
      <td className="cell">
        <span style={{ display: 'inline-flex', gap: 2 }}>
          <ToolButton icon="pencil" size={22} title={`Edit ${i.name}`} onClick={() => edit(i.name)} />
          <ToolButton icon="trash" size={22} danger title={`Take ${i.name} out of the library — asks first`} onClick={() => remove(i.name)} />
        </span>
      </td>
    </tr>
  );

  return (
    <table className="library-grid">
      <thead>
        <tr>
          <th rowSpan={2}>Instructions</th>
          <th rowSpan={2} title="The accounts each is meant for — every account, every one of a CLI, or those you name">
            Meant for
          </th>
          {PROVIDERS.map(provider => {
            const span = cols.filter(a => a.provider === provider).length;
            return span ? (
              <th key={provider} colSpan={span} className="group" title={`Which instructions each ${PROVIDER_LABEL[provider]} account gets — one at most`}>
                <span style={{ display: 'inline-flex', alignItems: 'center', gap: 3 }}>
                  <Icon name={provider} size={14} />
                  {PROVIDER_LABEL[provider]} gets
                </span>
              </th>
            ) : null;
          })}
          <th rowSpan={2} />
        </tr>
        <tr>
          {cols.map(a => (
            <th key={accountKey(a)} title={`What ${a.name} (${PROVIDER_LABEL[a.provider]}) gets`}>
              {a.name}
            </th>
          ))}
        </tr>
      </thead>
      <tbody>
        {o.instructions.map(row)}
        <tr>
          <td style={{ maxWidth: 0, width: '100%' }}>
            <div style={{ fontWeight: 700, color: 'var(--dim)' }}>None</div>
            <div
              title="Nothing from the library; the account’s own file, if any, is left alone"
              style={{ fontSize: 'var(--fs-2)', color: 'var(--faint)', overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}
            >
              Nothing from the library; the account’s own file, if any, is left alone
            </div>
          </td>
          <td />
          {cols.map(a => {
            const key = accountKey(a);
            const none = !(key in o.instructionsFor);
            return (
              <td key={key} className="cell">
                <Tick
                  id={`lib-ins-none-${key}`}
                  radio={`lib-ins-${key}`}
                  checked={none}
                  disabled={busy}
                  title={none ? `${a.name} gets no instructions from the library` : `Give ${a.name} no instructions`}
                  onChange={() => void act(librarySetInstructions(null, [...o.noInstructions, key]))}
                />
                <CellDot places={[]} />
              </td>
            );
          })}
          <td />
        </tr>
        {o.folders.map(f => (
          <tr key={f.path}>
            <td style={{ maxWidth: 0, width: '100%' }}>
              <div style={{ display: 'flex', alignItems: 'center', gap: 4, fontWeight: 700 }} title={f.path}>
                <Icon name="folder" size={14} />
                <span style={{ overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap', color: f.found ? undefined : RED }}>{baseName(f.path)}</span>
              </div>
              <div style={{ fontSize: 'var(--fs-2)', color: 'var(--faint)', overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
                The sessions Luna starts there, in place of the account’s
              </div>
            </td>
            <td />
            {cols.map(a => (
              <td key={accountKey(a)} className="cell">
                <FolderInstructions folder={f} account={a} names={o.instructions.map(i => i.name)} act={act} />
              </td>
            ))}
            <td />
          </tr>
        ))}
      </tbody>
    </table>
  );
}

/**
 * The user's own skills and instructions, kept in Luna's library and laid out
 * into the accounts each is meant for and for the sessions in folders named here. Raised from the Accounts
 * window, or from the status bar while a copy needs a look.
 */
export function LibraryWindow() {
  const open = useLibrary(s => s.open);
  return open ? <LibraryDialog onClose={() => useLibrary.getState().setOpen(false)} /> : null;
}

function LibraryDialog({ onClose }: { onClose: () => void }) {
  const host = useDialogHost();
  const me = useRef<HTMLDivElement>(null);
  const overview = useLibrary(s => s.overview);
  const error = useLibrary(s => s.error);
  // Commands still out: several can be, and the first back must not read as all done.
  const [pending, setPending] = useState(0);
  const busy = pending > 0;
  const [editing, setEditing] = useState<LibraryEntry | null>(null);
  const [deleting, setDeleting] = useState<LibraryEntry | null>(null);
  const [newName, setNewName] = useState('');
  const [newInstructions, setNewInstructions] = useState('');

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      // Only while this is the topmost window: the Accounts window can sit
      // under it, a confirmation over it.
      const all = document.querySelectorAll('.scrim');
      if (e.key === 'Escape' && all[all.length - 1] === me.current) onClose();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [onClose]);

  const act = (p: Promise<LibraryOverviewDto | null>) => {
    setPending(n => n + 1);
    return useLibrary.getState().run(p).finally(() => setPending(n => n - 1));
  };

  if (!host) return null;
  const o = overview;
  const look = o ? waiting(o.places) : [];
  const left = o ? o.places.filter(p => p.status === 'skipped') : [];
  const createSkill = () => {
    const name = newName.trim();
    if (!name) return;
    void act(libraryNewSkill(name)).then(ok => {
      if (!ok) return;
      setNewName('');
      setEditing({ skill: name });
    });
  };
  const createInstructions = () => {
    const name = newInstructions.trim();
    if (!name) return;
    void act(libraryNewInstructions(name)).then(ok => {
      if (!ok) return;
      setNewInstructions('');
      setEditing({ instructions: name });
    });
  };

  const dialog = (
    <div
      ref={me}
      className="scrim"
      onClick={e => {
        e.stopPropagation();
        onClose();
      }}
      style={{
        position: 'fixed', inset: 0, background: 'rgba(0,0,0,.35)', backdropFilter: 'blur(1px)',
        display: 'grid', placeItems: 'center', zIndex: 68
      }}
    >
      <div
        onClick={e => e.stopPropagation()}
        className="window"
        style={{ width: WINDOW_WIDTH + FOLDER_WIDTH * Math.max(0, (o?.folders.length ?? 0) - 1), maxWidth: 'calc(100vw - 32px)', boxShadow: 'var(--shadow), var(--border-window-outer), var(--border-window-inner)' }}
      >
        <div className="title-bar">
          <div className="title-bar-text">Skills &amp; instructions</div>
          <div className="title-bar-controls">
            <button aria-label="Close" onClick={onClose} />
          </div>
        </div>
        <div className="window-body" style={{ maxHeight: 'calc(100vh - 120px)', overflowY: 'auto' }}>
          {!o ? (
            <div style={{ fontSize: 'var(--fs-3)', color: error ? RED : 'var(--faint)' }}>{error ?? 'Reading the library…'}</div>
          ) : (
            <>
              <Section
                label="Library"
                aside={
                  <ToolButton icon="sync" label="Sync now" spin={busy} disabled={busy} title="Lay the library out now — everything that needs no asking" onClick={() => act(librarySync())} />
                }
              >
                <div style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
                  <Icon name="library" />
                  <div
                    title={o.root}
                    style={{ flex: 1, minWidth: 0, fontSize: 'var(--fs-3)', color: 'var(--dim)', whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis', direction: 'rtl', textAlign: 'left' }}
                  >
                    <bdi>{o.root}</bdi>
                  </div>
                  <button className="slim" onClick={() => void libraryOpen(null)} title="Open the library's folder — for files beside a skill's SKILL.md">
                    Open
                  </button>
                  <button className="slim" onClick={() => void pickFolder().then(p => { if (p) void act(librarySetSource(p)); })}>
                    Change…
                  </button>
                  <button className="slim" disabled={o.isDefault} title="Back to the library in Luna's own folder" onClick={() => act(librarySetSource(''))}>
                    Default
                  </button>
                </div>
                {!o.found && <Note>The folder is not there yet: nothing is laid out, and nothing taken out, until it is. Writing instructions or a new skill makes it.</Note>}
                {error && <div style={{ fontSize: 'var(--fs-2)', color: RED, marginTop: 4 }}>{error}</div>}
                {o.kept.length > 0 && (
                  <Note>
                    Left in place, no longer wanted but changed since Luna wrote them: {o.kept.join(', ')}
                  </Note>
                )}
              </Section>

              {look.length > 0 && (
                <Section label={`Needs a look — ${look.length}`}>
                  {look.map(({ place, count }) => (
                    <LookRow key={place.dest} place={place} count={count} act={act} />
                  ))}
                  <Note>
                    A copy changed where it lies is never replaced unasked. Whichever side is replaced is kept in Luna’s
                    backups for 30 days.
                  </Note>
                </Section>
              )}

              <Section label="Instructions" aside={o.instructions.length > 0 && <DotLegend />}>
                {o.instructions.length > 0 ? (
                  <InstructionsGrid
                    o={o}
                    busy={busy}
                    act={act}
                    edit={name => setEditing({ instructions: name })}
                    remove={name => setDeleting({ instructions: name })}
                  />
                ) : (
                  <Note>The library has no instructions yet — name some below{o.foundInstructions.length > 0 ? ', or start from an account’s own.' : '.'}</Note>
                )}
                <div style={{ display: 'flex', gap: 4, marginTop: 6 }}>
                  <input
                    type="text"
                    value={newInstructions}
                    placeholder="new-instructions-name"
                    onChange={e => setNewInstructions(e.target.value)}
                    onKeyDown={e => {
                      if (e.key === 'Enter') createInstructions();
                    }}
                    style={{ flex: 1, minWidth: 0 }}
                  />
                  <button className="slim" disabled={!newInstructions.trim() || busy} onClick={createInstructions}>
                    New instructions
                  </button>
                </div>
                {o.foundInstructions.length > 0 && (
                  <div style={{ display: 'flex', alignItems: 'center', gap: 6, flexWrap: 'wrap', marginTop: 6, fontSize: 'var(--fs-2)', color: 'var(--dim)' }}>
                    Has a file of its own, not from the library:
                    {o.foundInstructions.map(f => (
                      <button key={f.dest} className="slim" title={`${f.dest}\nBecomes instructions named after the account, meant for it alone`} onClick={() => act(libraryResolve(f.dest, true))}>
                        <span style={{ display: 'inline-flex', alignItems: 'center', gap: 4 }}>
                          <TargetLabel target={f.target} />— take into library
                        </span>
                      </button>
                    ))}
                  </div>
                )}
                {o.oldInstructions && (
                  <div style={{ display: 'flex', alignItems: 'center', gap: 6, fontSize: 'var(--fs-2)', color: 'var(--dim)', marginTop: 6 }} title={o.oldInstructions}>
                    <Icon name="warn" size={14} />
                    <span>
                      The library still has the <code>instructions.md</code> of before there could be several, beside{' '}
                      <code>instructions/main.md</code>: nothing is laid out from it. Move what you need into instructions here and delete it.
                    </span>
                  </div>
                )}
                {o.overridden.length > 0 && (
                  <div style={{ display: 'flex', alignItems: 'center', gap: 6, fontSize: 'var(--fs-2)', color: 'var(--dim)', marginTop: 6 }}>
                    <Icon name="warn" size={14} />
                    Codex reads <code>AGENTS.override.md</code> instead of <code>AGENTS.md</code> in {o.overridden.join(', ')}: the
                    library’s instructions do not reach it there.
                  </div>
                )}
                <Note>
                  Read by both CLIs at the start of every session — as <code>CLAUDE.md</code> in Claude Code,{' '}
                  <code>AGENTS.md</code> in Codex: the place for rules that always hold. An account gets one at most: where
                  several are meant for it, one that names it beats one for its CLI, which beats one for all. A radio gives an
                  account that one. Its own file is replaced only once you say so under Needs a look. A folder of the
                  library can give the Claude Code sessions Luna starts in it other instructions, or none: the account’s
                  file is left out of them at every start (<code>claudeMdExcludes</code>), the other ones given as a{' '}
                  <code>CLAUDE.md</code> of their own. Codex reads its account’s <code>AGENTS.md</code> in every session, with no way to leave
                  it out of one.
                </Note>
              </Section>

              <Section
                label="Skills"
                aside={
                  <>
                    {o.skills.length > 0 && <DotLegend />}
                    <AddFolder columns={o.folders.map(f => f.path)} onAdd={p => void act(librarySetFolder(p, true))} />
                  </>
                }
              >
                {o.skills.length > 0 && (
                  // Where the screen cannot give the window the width, the grid scrolls sooner than squeeze the names.
                  <div style={{ overflowX: 'auto' }}>
                    <table className="library-grid">
                      <thead>
                        <tr>
                          <th rowSpan={o.folders.length ? 2 : 1}>Skill</th>
                          <th rowSpan={o.folders.length ? 2 : 1} title="The accounts each is meant for — every account, every one of a CLI, or those you name">
                            Meant for
                          </th>
                          {o.folders.length > 0 && (
                            <th
                              colSpan={o.folders.length}
                              className="group"
                              title="The sessions Luna starts in these folders, their worktrees included: which accounts' get each skill there"
                            >
                              Sessions in
                            </th>
                          )}
                          <th rowSpan={o.folders.length ? 2 : 1} />
                        </tr>
                        {o.folders.length > 0 && (
                          <tr>
                            {o.folders.map(f => (
                              <th key={f.path} className="library-folder" title={f.found ? f.path : `${f.path}\nNot there`}>
                                <span style={{ display: 'inline-flex', alignItems: 'center', gap: 3, maxWidth: FOLDER_WIDTH - 12 }}>
                                  <Icon name="folder" size={14} />
                                  <span style={{ color: f.found ? undefined : RED, overflow: 'hidden', textOverflow: 'ellipsis' }}>{baseName(f.path)}</span>
                                  <ToolButton
                                    icon="close"
                                    size={18}
                                    title={`Stop giving skills to sessions in ${f.path}\nWhat waits for them goes, those copies still as Luna wrote them`}
                                    onClick={() => act(librarySetFolder(f.path, false))}
                                  />
                                </span>
                              </th>
                            ))}
                          </tr>
                        )}
                      </thead>
                      <tbody>
                        {o.skills.map(s => {
                          const mine = o.places.filter(p => p.skill === s.name);
                          return (
                            <tr key={s.name}>
                              <td style={{ maxWidth: 0, minWidth: 200, width: '100%' }}>
                                <NameCell name={s.name} />
                                <div
                                  title={s.problem ?? s.description}
                                  style={{ fontSize: 'var(--fs-2)', color: s.problem ? RED : 'var(--faint)', overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}
                                >
                                  {s.problem ?? s.description}
                                </div>
                              </td>
                              <td className="cell">
                                <ReachPicker
                                  id={`lib-${s.name}-accounts`}
                                  reach={s.accounts}
                                  accounts={o.accounts}
                                  onChange={r => act(librarySetAccounts(s.name, r ?? []))}
                                />
                                <CellDot places={mine.filter(p => p.target.kind === 'account')} />
                              </td>
                              {o.folders.map((f, i) => (
                                <td key={f.path} className="cell">
                                  <FolderCell id={`lib-${s.name}-folder-${i}`} skill={s.name} folder={f} accounts={o.accounts} act={act} />
                                  <CellDot places={mine.filter(p => p.target.kind === 'folder' && p.target.path === f.path)} />
                                </td>
                              ))}
                              <td className="cell">
                                <span style={{ display: 'inline-flex', gap: 2 }}>
                                  <ToolButton icon="pencil" size={22} title={`Edit ${s.name}'s SKILL.md`} onClick={() => setEditing({ skill: s.name })} />
                                  <ToolButton icon="folder" size={22} title={`Open ${s.name}'s folder — for the files beside SKILL.md`} onClick={() => void libraryOpen(s.name)} />
                                  <ToolButton icon="trash" size={22} danger title={`Take ${s.name} out of the library — asks first`} onClick={() => setDeleting({ skill: s.name })} />
                                </span>
                              </td>
                            </tr>
                          );
                        })}
                      </tbody>
                    </table>
                  </div>
                )}
                <div style={{ display: 'flex', gap: 4, marginTop: 6 }}>
                  <input
                    type="text"
                    value={newName}
                    placeholder="new-skill-name"
                    onChange={e => setNewName(e.target.value)}
                    onKeyDown={e => {
                      if (e.key === 'Enter') createSkill();
                    }}
                    style={{ flex: 1, minWidth: 0 }}
                  />
                  <button className="slim" disabled={!newName.trim() || busy} onClick={createSkill}>
                    New skill
                  </button>
                </div>
                <Note>
                  A skill is a folder with a <code>SKILL.md</code>, whose description is all the model sees until it opens it:
                  the place for procedures and recipes. It goes into the <code>skills/</code> of each account it is meant for.
                  A folder can pick other accounts for the sessions Luna starts in it, worktrees included: a tick is every
                  account, a dash some, a cross none; faint, it picks nothing and goes by what holds outside it. Given at
                  their start, nothing written into the folder; kept from them, hidden at their start, so a CLI started
                  outside Luna there still has an account’s copy. In a folder inside another, the inner one decides. Codex
                  lists a folder's skills as{' '}
                  <code>&lt;folder&gt;:&lt;name&gt;</code>; for Claude Code a copy whose name a skill there already has goes by{' '}
                  <code>luna-&lt;name&gt;</code>.
                </Note>
              </Section>

              {left.length > 0 && (
                <Section label="Left out">
                  {left.map(p => (
                    <div key={p.dest} style={{ display: 'flex', alignItems: 'center', gap: 8, borderTop: '1px solid var(--line)', padding: '5px 0' }}>
                      <TargetLabel target={p.target} />
                      <b>{what(p)}</b>
                      <span style={{ flex: 1 }} />
                      <button className="slim" onClick={() => act(librarySkip(p.dest, false))}>
                        Take back in
                      </button>
                    </div>
                  ))}
                </Section>
              )}
            </>
          )}
        </div>
      </div>
    </div>
  );

  // The editor and the confirmation stand beside the window, not inside it:
  // a click in a portal still bubbles through its React parents, and the
  // window's backdrop closes on one.
  return (
    <>
      {createPortal(dialog, host)}
      {editing && <LibraryEditor entry={editing} onClose={() => setEditing(null)} />}
      {deleting && 'skill' in deleting && (
        <ConfirmDialog
          title="Take the skill out"
          body={
            <>
              Moves <b>{deleting.skill}</b> out of the library into Luna’s backups, and takes its copies out wherever they are
              still exactly as Luna wrote them. A copy changed since stays where it is.
            </>
          }
          confirmLabel="Take out"
          onCancel={() => setDeleting(null)}
          onConfirm={() => {
            act(libraryDeleteSkill(deleting.skill));
            setDeleting(null);
          }}
        />
      )}
      {deleting && 'instructions' in deleting && (
        <ConfirmDialog
          title="Take the instructions out"
          body={
            <>
              Moves <b>{deleting.instructions}</b> out of the library into Luna’s backups. The accounts it went into get
              the next instructions that take them in, or none: its copies are replaced or taken out wherever they are
              still exactly as Luna wrote them. A copy changed since stays where it is.
            </>
          }
          confirmLabel="Take out"
          onCancel={() => setDeleting(null)}
          onConfirm={() => {
            act(libraryDeleteInstructions(deleting.instructions));
            setDeleting(null);
          }}
        />
      )}
    </>
  );
}
