import { useEffect, useState } from 'react';
import type { SystemSkillState } from '../../ipc/applied';
import {
  librarySystemAdopt,
  librarySystemCompare,
  librarySystemReset,
  type EditorEntry,
  type LibraryEntry,
  type LibraryOverviewDto,
  type LibrarySkillDto,
  type SystemCompareDto
} from '../../ipc/library';
import { ConfirmDialog } from '../../shared/ui/ConfirmDialog';
import { Icon, ToolButton } from '../../shared/ui/icons';
import { NameCell, Note } from './cells';
import { Changes } from './DiffView';
import { GREEN, RED } from './presentation';

/** What each system skill is, as the window says it. */
const WHAT: Record<string, string> = {
  'session-handoff':
    'What a chat with Auto-handoff on is given when it writes its handoff. Luna serves it only to such chats, at the threshold; it is never put into an account or a folder.'
};

const short = (sha: string) => sha.slice(0, 12);

/** Whose text a system skill is served as now, in words. */
export function stateWords(s: SystemSkillState): string {
  switch (s.state) {
    case 'library':
      return `Yours: ${s.path}, sha ${short(s.sha256)}`;
    case 'broken':
      return `Yours is broken: ${s.problem ?? 'it will not do'}. Luna’s own is used (${s.version}, sha ${short(s.sha256)})`;
    default:
      return `Luna’s own: ${s.version}, sha ${short(s.sha256)}`;
  }
}

/** Luna's own text beside the override, as a diff; asked again whenever what is served changes (`key`). */
function Compare({ name }: { name: string }) {
  const [c, setC] = useState<SystemCompareDto | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let stale = false;
    librarySystemCompare(name)
      .then(r => !stale && setC(r))
      .catch((e: unknown) => !stale && setError(String(e)));
    return () => {
      stale = true;
    };
  }, [name]);
  const say = (text: string, color = 'var(--faint)') => (
    <div style={{ fontSize: 'var(--fs-2)', color, marginTop: 6 }}>{text}</div>
  );
  if (error) return say(error, RED);
  if (!c) return say('Reading…');
  if (c.yours === null) return say('There is no override: Luna’s own is served.');
  if (c.yours === c.luna) return say('Yours is Luna’s own, word for word.');
  return <Changes label="Luna’s own → yours" from={c.luna} to={c.yours} />;
}

/** One system skill: what it is, whose text is served, and what can be done about it. */
function SystemSkill({
  s,
  act,
  edit
}: {
  s: SystemSkillState;
  act: (p: Promise<LibraryOverviewDto | null>) => Promise<boolean>;
  edit: (entry: EditorEntry) => void;
}) {
  const [comparing, setComparing] = useState(false);
  const [resetting, setResetting] = useState(false);
  const mine = s.state !== 'luna';
  return (
    <div className="library-system" data-state={s.state}>
      <div style={{ display: 'flex', alignItems: 'center', gap: 8, flexWrap: 'wrap' }}>
        <NameCell name={s.name} />
        <span className="library-system-tag" title="Luna’s own, served at its own trigger; one override for everyone">
          System
        </span>
        <span style={{ flex: 1 }} />
        <button
          className="slim"
          title={
            mine
              ? `Edit your override of ${s.name}`
              : `Write your own ${s.name}, starting from Luna’s text: one override for every chat it is served to`
          }
          onClick={() => edit({ system: s.name })}
        >
          Edit
        </button>
        <button
          className="slim"
          disabled={!mine}
          title={mine ? 'Your override beside Luna’s own text' : 'There is no override to compare'}
          onClick={() => setComparing(!comparing)}
        >
          {comparing ? 'Hide comparison' : 'Compare with Luna’s'}
        </button>
        <button
          className="slim"
          disabled={!mine}
          title={
            mine ? 'Your override goes to Luna’s backups, and Luna’s own is served' : 'Luna’s own is what is served'
          }
          onClick={() => setResetting(true)}
        >
          Back to Luna’s
        </button>
      </div>
      {WHAT[s.name] && (
        <div style={{ fontSize: 'var(--fs-2)', color: 'var(--dim)', marginTop: 4, lineHeight: 1.4 }}>
          {WHAT[s.name]}
        </div>
      )}
      <div
        style={{ display: 'flex', alignItems: 'center', gap: 6, marginTop: 4, fontSize: 'var(--fs-2)' }}
        title={mine ? s.path : undefined}
      >
        {s.state === 'broken' ? (
          <Icon name="warn" size={14} />
        ) : (
          <span
            style={{
              width: 8,
              height: 8,
              borderRadius: '50%',
              background: GREEN,
              flex: 'none',
              display: 'inline-block'
            }}
          />
        )}
        <span
          style={{
            color: s.state === 'broken' ? 'var(--wait)' : 'var(--dim)',
            minWidth: 0,
            overflowWrap: 'anywhere'
          }}
        >
          {stateWords(s)}
        </span>
      </div>
      {comparing && mine && <Compare key={`${s.state}-${s.sha256}-${s.problem ?? ''}`} name={s.name} />}
      {resetting && (
        <ConfirmDialog
          title="Back to Luna’s own"
          body={
            <>
              Moves your override of <b>{s.name}</b> out of the library into Luna’s backups, kept for 30 days. From then
              on, chats are served Luna’s own text of this version.
            </>
          }
          confirmLabel="Back to Luna’s"
          onCancel={() => setResetting(false)}
          onConfirm={() => {
            setResetting(false);
            setComparing(false);
            void act(librarySystemReset(s.name));
          }}
        />
      )}
    </div>
  );
}

/**
 * The library's system skills, apart from your own: text Luna serves at its
 * own trigger and never lays out (library/system.rs). Each has one override
 * for everyone, so no accounts, no audience and no folders.
 */
export function SystemSkills({
  o,
  act,
  edit
}: {
  o: LibraryOverviewDto;
  act: (p: Promise<LibraryOverviewDto | null>) => Promise<boolean>;
  edit: (entry: EditorEntry) => void;
}) {
  return (
    <>
      {o.system.map(s => (
        <SystemSkill key={s.name} s={s} act={act} edit={edit} />
      ))}
      <Note>
        Luna’s own text, built in and of its version, unless you write your own: one override in the library, under{' '}
        <code>system/</code>, for every chat it is served to. One that will not do is not used: Luna’s own is, and the
        chat’s What Luna applied says why. A skill of the same name among yours, in a project or in an account is never
        served in its place.
      </Note>
    </>
  );
}

/**
 * A skill among yours that takes a system skill's name: laid out nowhere, and
 * not the override (library/system.rs `reserved`). Says so in the grid, and
 * offers to make it the override, its folder moved there.
 */
export function ReservedSkillRow({
  s,
  o,
  act,
  edit,
  remove
}: {
  s: LibrarySkillDto;
  o: LibraryOverviewDto;
  act: (p: Promise<LibraryOverviewDto | null>) => Promise<boolean>;
  edit: (entry: LibraryEntry) => void;
  remove: (entry: LibraryEntry) => void;
}) {
  const [adopting, setAdopting] = useState(false);
  const replaces = o.system.find(x => x.name === s.name)?.state !== 'luna';
  return (
    <tr data-reserved="yes">
      <td style={{ maxWidth: 0, minWidth: 200, width: '100%' }}>
        <NameCell name={s.name} />
        <div style={{ fontSize: 'var(--fs-2)', color: 'var(--wait)', lineHeight: 1.4 }}>{s.problem}</div>
      </td>
      <td className="cell" colSpan={2 + o.folders.length}>
        <button
          className="slim"
          title="Move this skill's folder to where the system skill's override goes — asks first"
          onClick={() => setAdopting(true)}
        >
          Make it the override…
        </button>
        {adopting && (
          <ConfirmDialog
            title="Make it the override"
            body={
              <>
                Moves <code>skills/{s.name}</code>, with any files beside its <code>SKILL.md</code>, to{' '}
                <code>system/{s.name}</code>, where its <code>SKILL.md</code> is the override every chat with
                Auto-handoff on is served in place of Luna’s own.{' '}
                {replaces && <>The override there now goes to Luna’s backups first, kept for 30 days. </>}
                It is no longer among your skills.
              </>
            }
            confirmLabel="Make it the override"
            danger={false}
            onCancel={() => setAdopting(false)}
            onConfirm={() => {
              setAdopting(false);
              void act(librarySystemAdopt(s.name));
            }}
          />
        )}
      </td>
      <td className="cell">
        <span style={{ display: 'inline-flex', gap: 2 }}>
          <ToolButton
            icon="pencil"
            size={22}
            title={`Edit ${s.name}'s SKILL.md`}
            onClick={() => edit({ skill: s.name })}
          />
          <ToolButton
            icon="trash"
            size={22}
            danger
            title={`Take ${s.name} out of the library — asks first`}
            onClick={() => remove({ skill: s.name })}
          />
        </span>
      </td>
    </tr>
  );
}
