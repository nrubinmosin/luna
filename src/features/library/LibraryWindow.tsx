import { useEffect, useRef, useState, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import {
  libraryDeleteInstructions, libraryDeleteSkill, libraryNewInstructions, libraryNewSkill, libraryOpen,
  libraryRenameInstructions, libraryResolve, librarySetAccounts, librarySetFolder, librarySetInstructions,
  librarySetSource, librarySkip, librarySync, pickFolder,
  type LibraryEntry, type LibraryInstructionsDto, type LibraryOverviewDto, type LibraryTarget, type PlaceDto, type Reach
} from '../../ipc/commands';
import { useDialogHost } from '../../shared/ui/dialogHost';
import { ConfirmDialog } from '../../shared/ui/ConfirmDialog';
import { Icon, ToolButton } from '../../shared/ui/icons';
import { PROVIDER_LABEL, type Provider } from '../../shared/types';
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

/** Whose a copy is: an account by its CLI's icon and name, a folder by its name. */
function TargetLabel({ target }: { target: LibraryTarget }) {
  return target.kind === 'account' ? (
    <span style={{ display: 'inline-flex', alignItems: 'center', gap: 4, whiteSpace: 'nowrap' }} title={`${PROVIDER_LABEL[target.provider]} account`}>
      <Icon name={target.provider} size={14} />
      {target.name}
    </span>
  ) : (
    <span style={{ display: 'inline-flex', alignItems: 'center', gap: 4, whiteSpace: 'nowrap' }} title={target.path}>
      <Icon name="folder" size={14} />
      {baseName(target.path)}
    </span>
  );
}

const what = (p: PlaceDto) => p.skill ?? (p.instructions ? `${p.instructions} instructions` : 'Instructions');

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
function Tick({ id, radio, checked, disabled, title, onChange }: {
  id: string;
  /** The group a radio belongs to. */
  radio?: string;
  checked: boolean;
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
        disabled={disabled}
        onChange={e => onChange(e.target.checked)}
      />
      <label htmlFor={id} title={title} style={{ display: 'inline-block', width: 0, height: 16, marginLeft: 18, verticalAlign: 'middle' }} />
    </>
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
    .map(p => `${p.target.kind === 'account' ? p.target.name : baseName(p.target.path)}: ${needsLook(p) ? sayStatus(p) : p.status}`)
    .join('\n');
  return <span title={title} style={{ width: 8, height: 8, borderRadius: '50%', background: color, display: 'inline-block', marginLeft: 4 }} />;
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
          <div style={{ fontWeight: 700 }} title="Double-click to rename" onDoubleClick={() => setRenaming({ from: i.name, to: i.name })}>
            {i.name}
          </div>
        )}
        <div
          title={i.problem ?? i.description}
          style={{ fontSize: 'var(--fs-2)', color: i.problem ? 'var(--wait)' : 'var(--faint)', overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}
        >
          {i.problem ?? i.description}
        </div>
      </td>
      <td className="cell">
        <ReachPicker id={`lib-ins-${i.name}`} reach={i.accounts} accounts={o.accounts} onChange={r => act(librarySetInstructions(i.name, r))} />
      </td>
      {o.accounts.map(a => {
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
          <ToolButton icon="file" size={22} title={`Edit ${i.name}`} onClick={() => edit(i.name)} />
          <ToolButton icon="pencil" size={22} title={`Rename ${i.name}`} onClick={() => setRenaming({ from: i.name, to: i.name })} />
          <ToolButton icon="trash" size={22} danger title={`Take ${i.name} out of the library — asks first`} onClick={() => remove(i.name)} />
        </span>
      </td>
    </tr>
  );

  return (
    <table className="library-grid">
      <thead>
        <tr>
          <th>Instructions</th>
          <th title="The accounts each goes into — every account, every one of a CLI, or those you name">Accounts</th>
          {o.accounts.map(a => (
            <th key={accountKey(a)} title={`What ${a.name} (${PROVIDER_LABEL[a.provider]}) gets`}>
              <span style={{ display: 'inline-flex', alignItems: 'center', gap: 3 }}>
                <Icon name={a.provider} size={14} />
                {a.name}
              </span>
            </th>
          ))}
          <th />
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
          {o.accounts.map(a => {
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
      </tbody>
    </table>
  );
}

/**
 * The user's own skills and instructions, kept in Luna's library and laid out
 * into the accounts each is meant for and into folders named here. Raised from the Accounts
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
        style={{ width: 760, maxWidth: 'calc(100vw - 32px)', boxShadow: 'var(--shadow), var(--border-window-outer), var(--border-window-inner)' }}
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
                    A copy changed where it lies is never replaced unasked. Taking it makes it the library’s and every
                    other copy’s; putting the library’s back replaces it. Whichever side is replaced is kept in Luna’s
                    backups for 30 days.
                  </Note>
                </Section>
              )}

              <Section label="Instructions">
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
                    An account’s own file, where no instructions go — take it in as new instructions for that account:
                    {o.foundInstructions.map(f => (
                      <button key={f.dest} className="slim" title={`${f.dest}\nBecomes instructions named after the account, going into it alone`} onClick={() => act(libraryResolve(f.dest, true))}>
                        <TargetLabel target={f.target} />
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
                  An instructions file serves both CLIs — <code>CLAUDE.md</code> for Claude Code, <code>AGENTS.md</code> for
                  Codex — read at the start of every session: the place for rules that always hold. An account gets one at
                  most: where several take it in, the one that names it wins over every one of its CLI, and that over every
                  account. An account’s own file is replaced only once you take it or replace it under Needs a look.
                </Note>
              </Section>

              <Section
                label="Skills"
                aside={
                  <ToolButton
                    icon="folder"
                    label="Add folder"
                    title="A folder the ticked skills go into, each as <folder>/<name>"
                    onClick={() =>
                      void pickFolder().then(p => {
                        if (p && !o.folders.some(f => f.path.toLowerCase() === p.toLowerCase())) act(librarySetFolder(p, []));
                      })
                    }
                  />
                }
              >
                {o.skills.length > 0 && (
                  <table className="library-grid">
                    <thead>
                      <tr>
                        <th>Skill</th>
                        <th title="The Claude Code and Codex accounts it goes into">Accounts</th>
                        {o.folders.map(f => (
                          <th key={f.path} title={f.path}>
                            <span style={{ display: 'inline-flex', alignItems: 'center', gap: 2 }}>
                              <span style={{ color: f.found ? undefined : RED }}>{baseName(f.path)}</span>
                              <ToolButton
                                icon="close"
                                size={18}
                                title={`Stop laying skills out into ${f.path}\nLuna's copies there go, those still as it wrote them`}
                                onClick={() => act(librarySetFolder(f.path, null))}
                              />
                            </span>
                          </th>
                        ))}
                        <th />
                      </tr>
                    </thead>
                    <tbody>
                      {o.skills.map(s => {
                        const mine = o.places.filter(p => p.skill === s.name);
                        return (
                          <tr key={s.name}>
                            <td style={{ maxWidth: 0, width: '100%' }}>
                              <div style={{ fontWeight: 700 }}>{s.name}</div>
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
                                onChange={r => act(librarySetAccounts(s.name, r))}
                              />
                              <CellDot places={mine.filter(p => p.target.kind === 'account')} />
                            </td>
                            {o.folders.map((f, i) => (
                              <td key={f.path} className="cell">
                                <Tick
                                  id={`lib-${s.name}-folder-${i}`}
                                  title={`${s.name} into ${f.path}`}
                                  checked={f.skills.includes(s.name)}
                                  // A tick sends the folder's whole list: one made before the last
                                  // answer is in would build on the list without the last tick.
                                  disabled={busy}
                                  onChange={on => act(librarySetFolder(f.path, on ? [...f.skills, s.name] : f.skills.filter(n => n !== s.name)))}
                                />
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
                  A skill is a folder with a <code>SKILL.md</code>: its description is all the model sees until it decides
                  to open it — the place for procedures and recipes. Into the accounts picked for it — every account,
                  every one of a CLI, or those you name — it goes into each one’s <code>skills/</code>; ticked for a folder it goes into that folder as <code>&lt;folder&gt;/&lt;name&gt;</code>, and how a
                  CLI gets to it from there — a link into <code>.claude/skills</code> or <code>.agents/skills</code>, a git
                  ignore — is yours to set up. Nothing else in an account or a folder is touched.
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
