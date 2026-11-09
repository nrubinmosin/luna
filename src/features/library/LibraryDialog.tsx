import { useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { pickFolder } from '../../ipc/cli';
import {
  libraryDeleteInstructions,
  libraryDeleteSkill,
  libraryNewInstructions,
  libraryNewSkill,
  libraryOpen,
  libraryResolve,
  librarySetFolder,
  librarySetSource,
  librarySkip,
  librarySync,
  type LibraryEntry,
  type LibraryOverviewDto
} from '../../ipc/library';
import { useDialogHost } from '../../shared/ui/dialogHost';
import { ConfirmDialog } from '../../shared/ui/ConfirmDialog';
import { Icon, ToolButton } from '../../shared/ui/icons';
import { Section } from '../../shared/ui/Section';
import { AddFolder } from './AddFolder';
import { DotLegend, Note, TargetLabel } from './cells';
import { what } from './places';
import { FOLDER_WIDTH, RED, WINDOW_WIDTH } from './presentation';
import { InstructionsGrid } from './InstructionsGrid';
import { LibraryEditor } from './LibraryEditor';
import { useLibrary, waiting } from './library.store';
import { LookRow } from './LookRow';
import { SkillsGrid } from './SkillsGrid';

export function LibraryDialog({ onClose }: { onClose: () => void }) {
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
    return useLibrary
      .getState()
      .run(p)
      .finally(() => setPending(n => n - 1));
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
        position: 'fixed',
        inset: 0,
        background: 'rgba(0,0,0,.35)',
        backdropFilter: 'blur(1px)',
        display: 'grid',
        placeItems: 'center',
        zIndex: 68
      }}
    >
      <div
        onClick={e => e.stopPropagation()}
        className="window"
        style={{
          width: WINDOW_WIDTH + FOLDER_WIDTH * Math.max(0, (o?.folders.length ?? 0) - 1),
          boxShadow: 'var(--shadow), var(--border-window-outer), var(--border-window-inner)'
        }}
      >
        <div className="title-bar">
          <div className="title-bar-text">Skills &amp; instructions</div>
          <div className="title-bar-controls">
            <button aria-label="Close" onClick={onClose} />
          </div>
        </div>
        <div className="window-body">
          {!o ? (
            <div style={{ fontSize: 'var(--fs-3)', color: error ? RED : 'var(--faint)' }}>
              {error ?? 'Reading the library…'}
            </div>
          ) : (
            <>
              <Section
                label="Library"
                aside={
                  <ToolButton
                    icon="sync"
                    label="Sync now"
                    spin={busy}
                    disabled={busy}
                    title="Lay the library out now — everything that needs no asking"
                    onClick={() => void act(librarySync())}
                  />
                }
              >
                <div style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
                  <Icon name="library" />
                  <div
                    title={o.root}
                    style={{
                      flex: 1,
                      minWidth: 0,
                      fontSize: 'var(--fs-3)',
                      color: 'var(--dim)',
                      whiteSpace: 'nowrap',
                      overflow: 'hidden',
                      textOverflow: 'ellipsis',
                      direction: 'rtl',
                      textAlign: 'left'
                    }}
                  >
                    <bdi>{o.root}</bdi>
                  </div>
                  <button
                    className="slim"
                    onClick={() => void libraryOpen(null)}
                    title="Open the library's folder — for files beside a skill's SKILL.md"
                  >
                    Open
                  </button>
                  <button
                    className="slim"
                    onClick={() =>
                      void pickFolder().then(p => {
                        if (p) void act(librarySetSource(p));
                      })
                    }
                  >
                    Change…
                  </button>
                  <button
                    className="slim"
                    disabled={o.isDefault}
                    title="Back to the library in Luna's own folder"
                    onClick={() => void act(librarySetSource(''))}
                  >
                    Default
                  </button>
                </div>
                {!o.found && (
                  <Note>
                    The folder is not there yet: nothing is laid out, and nothing taken out, until it is. Writing
                    instructions or a new skill makes it.
                  </Note>
                )}
                {error && <div style={{ fontSize: 'var(--fs-2)', color: RED, marginTop: 4 }}>{error}</div>}
                {o.kept.length > 0 && (
                  <Note>Left in place, no longer wanted but changed since Luna wrote them: {o.kept.join(', ')}</Note>
                )}
              </Section>

              {look.length > 0 && (
                <Section label={`Needs a look — ${look.length}`}>
                  {look.map(({ place, count }) => (
                    <LookRow key={place.dest} place={place} count={count} act={p => void act(p)} />
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
                  <Note>
                    The library has no instructions yet — name some below
                    {o.foundInstructions.length > 0 ? ', or start from an account’s own.' : '.'}
                  </Note>
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
                  <div
                    style={{
                      display: 'flex',
                      alignItems: 'center',
                      gap: 6,
                      flexWrap: 'wrap',
                      marginTop: 6,
                      fontSize: 'var(--fs-2)',
                      color: 'var(--dim)'
                    }}
                  >
                    Has a file of its own, not from the library:
                    {o.foundInstructions.map(f => (
                      <button
                        key={f.dest}
                        className="slim"
                        title={`${f.dest}\nBecomes instructions named after the account, meant for it alone`}
                        onClick={() => void act(libraryResolve(f.dest, true))}
                      >
                        <span style={{ display: 'inline-flex', alignItems: 'center', gap: 4 }}>
                          <TargetLabel target={f.target} />— take into library
                        </span>
                      </button>
                    ))}
                  </div>
                )}
                {o.oldInstructions && (
                  <div
                    style={{
                      display: 'flex',
                      alignItems: 'center',
                      gap: 6,
                      fontSize: 'var(--fs-2)',
                      color: 'var(--dim)',
                      marginTop: 6
                    }}
                    title={o.oldInstructions}
                  >
                    <Icon name="warn" size={14} />
                    <span>
                      The library still has the <code>instructions.md</code> of before there could be several, beside{' '}
                      <code>instructions/main.md</code>: nothing is laid out from it. Move what you need into
                      instructions here and delete it.
                    </span>
                  </div>
                )}
                {o.overridden.length > 0 && (
                  <div
                    style={{
                      display: 'flex',
                      alignItems: 'center',
                      gap: 6,
                      fontSize: 'var(--fs-2)',
                      color: 'var(--dim)',
                      marginTop: 6
                    }}
                  >
                    <Icon name="warn" size={14} />
                    Codex reads <code>AGENTS.override.md</code> instead of <code>AGENTS.md</code> in{' '}
                    {o.overridden.join(', ')}: the library’s instructions do not reach it there.
                  </div>
                )}
                <Note>
                  Read by both CLIs at the start of every session — as <code>CLAUDE.md</code> in Claude Code,{' '}
                  <code>AGENTS.md</code> in Codex: the place for rules that always hold. An account gets one at most:
                  where several are meant for it, one that names it beats one for its CLI, which beats one for all. A
                  radio gives an account that one. Its own file is replaced only once you say so under Needs a look. A
                  folder of the library can give the Claude Code sessions Luna starts in it other instructions, or none:
                  the account’s file is left out of them at every start (<code>claudeMdExcludes</code>), the other ones
                  given as a <code>CLAUDE.md</code> of their own. Codex reads its account’s <code>AGENTS.md</code> in
                  every session, with no way to leave it out of one.
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
                {o.skills.length > 0 && <SkillsGrid o={o} act={act} edit={setEditing} remove={setDeleting} />}
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
                  A skill is a folder with a <code>SKILL.md</code>, whose description is all the model sees until it
                  opens it: the place for procedures and recipes. It goes into the <code>skills/</code> of each account
                  it is meant for. A folder can pick other accounts for the sessions Luna starts in it, worktrees
                  included: a tick is every account, a dash some, a cross none; faint, it picks nothing and goes by what
                  holds outside it. Given at their start, nothing written into the folder; kept from them, hidden at
                  their start, so a CLI started outside Luna there still has an account’s copy. In a folder inside
                  another, the inner one decides. Codex lists a folder's skills as{' '}
                  <code>&lt;folder&gt;:&lt;name&gt;</code>; for Claude Code a copy whose name a skill there already has
                  goes by <code>luna-&lt;name&gt;</code>.
                </Note>
              </Section>

              {left.length > 0 && (
                <Section label="Left out">
                  {left.map(p => (
                    <div
                      key={p.dest}
                      style={{
                        display: 'flex',
                        alignItems: 'center',
                        gap: 8,
                        borderTop: '1px solid var(--line)',
                        padding: '5px 0'
                      }}
                    >
                      <TargetLabel target={p.target} />
                      <b>{what(p)}</b>
                      <span style={{ flex: 1 }} />
                      <button className="slim" onClick={() => void act(librarySkip(p.dest, false))}>
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
              Moves <b>{deleting.skill}</b> out of the library into Luna’s backups, and takes its copies out wherever
              they are still exactly as Luna wrote them. A copy changed since stays where it is.
            </>
          }
          confirmLabel="Take out"
          onCancel={() => setDeleting(null)}
          onConfirm={() => {
            void act(libraryDeleteSkill(deleting.skill));
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
            void act(libraryDeleteInstructions(deleting.instructions));
            setDeleting(null);
          }}
        />
      )}
    </>
  );
}
