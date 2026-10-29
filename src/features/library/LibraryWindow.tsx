import { useEffect, useRef, useState, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import {
  libraryDeleteSkill, libraryNewSkill, libraryOpen, libraryResolve, librarySetAccounts, librarySetFolder,
  librarySetInstructions, librarySetSource, librarySkip, librarySync, pickFolder,
  type LibraryOverviewDto, type LibraryTarget, type PlaceDto
} from '../../ipc/commands';
import { useDialogHost } from '../../shared/ui/dialogHost';
import { ConfirmDialog } from '../../shared/ui/ConfirmDialog';
import { Icon, ToolButton } from '../../shared/ui/icons';
import { PROVIDER_LABEL } from '../../shared/types';
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

const what = (p: PlaceDto) => p.skill ?? 'Instructions';

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

/** A checkbox with no words beside it: the theme draws the box on a label. */
function Tick({ id, checked, disabled, title, onChange }: {
  id: string;
  checked: boolean;
  disabled: boolean;
  title: string;
  onChange: (on: boolean) => void;
}) {
  return (
    <>
      <input type="checkbox" id={id} checked={checked} disabled={disabled} onChange={e => onChange(e.target.checked)} />
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
  const [editing, setEditing] = useState<{ skill: string | null } | null>(null);
  const [deleting, setDeleting] = useState<string | null>(null);
  const [newName, setNewName] = useState('');

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
  const instructionPlaces = o ? o.places.filter(p => p.skill === null) : [];
  const createSkill = () => {
    const name = newName.trim();
    if (!name) return;
    setPending(n => n + 1);
    void useLibrary
      .getState()
      .run(libraryNewSkill(name))
      .then(ok => {
        if (!ok) return;
        setNewName('');
        setEditing({ skill: name });
      })
      .finally(() => setPending(n => n - 1));
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

              <Section
                label="Instructions"
                aside={
                  <ToolButton icon="pencil" label="Edit" title="Edit the library's instructions" onClick={() => setEditing({ skill: null })} />
                }
              >
                <div style={{ display: 'flex', alignItems: 'center', gap: 8, flexWrap: 'wrap', fontSize: 'var(--fs-3)' }}>
                  Into
                  <ReachPicker id="lib-instructions" reach={o.instructions} accounts={o.accounts} onChange={r => act(librarySetInstructions(r))} />
                  <span>
                    — <code>CLAUDE.md</code> for Claude Code, <code>AGENTS.md</code> for Codex
                  </span>
                  {o.instructions.length > 0 && instructionPlaces.length > 0 && (
                    <span style={{ fontSize: 'var(--fs-2)', color: 'var(--faint)' }}>
                      {instructionPlaces.filter(p => p.status === 'current').length} of {instructionPlaces.length} accounts up to date
                    </span>
                  )}
                </div>
                {!o.hasInstructions && (
                  <Note>
                    The library has no instructions yet — write them
                    {o.foundInstructions.length > 0 ? ', or start from an account’s own:' : '.'}
                  </Note>
                )}
                {!o.hasInstructions && o.foundInstructions.length > 0 && (
                  <div style={{ display: 'flex', gap: 6, flexWrap: 'wrap', marginTop: 4 }}>
                    {o.foundInstructions.map(f => (
                      <button key={f.dest} className="slim" title={f.dest} onClick={() => act(libraryResolve(f.dest, true))}>
                        <TargetLabel target={f.target} />
                      </button>
                    ))}
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
                  One file for both CLIs, read at the start of every session on the accounts it goes into — every
                  account, every one of a CLI, or those you pick — the place for rules that always hold. An account’s
                  own file is replaced only once you take it or replace it below.
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
                                <ToolButton icon="trash" size={22} danger title={`Take ${s.name} out of the library — asks first`} onClick={() => setDeleting(s.name)} />
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
      {editing && <LibraryEditor skill={editing.skill} onClose={() => setEditing(null)} />}
      {deleting && (
        <ConfirmDialog
          title="Take the skill out"
          body={
            <>
              Moves <b>{deleting}</b> out of the library into Luna’s backups, and takes its copies out wherever they are
              still exactly as Luna wrote them. A copy changed since stays where it is.
            </>
          }
          confirmLabel="Take out"
          onCancel={() => setDeleting(null)}
          onConfirm={() => {
            act(libraryDeleteSkill(deleting));
            setDeleting(null);
          }}
        />
      )}
    </>
  );
}
