import {
  libraryOpen,
  librarySetAccounts,
  librarySetFolder,
  type LibraryEntry,
  type LibraryOverviewDto
} from '../../ipc/library';
import { Icon, ToolButton } from '../../shared/ui/icons';
import { AUDIENCE_TITLE, AudiencePicker } from './AudiencePicker';
import { CellDot, NameCell } from './cells';
import { baseName } from './paths';
import { FOLDER_WIDTH, RED } from './presentation';
import { FolderCell } from './FolderCell';
import { ReachPicker } from './ReachPicker';
import { ReservedSkillRow } from './SystemSkills';

/**
 * The library's skills against where each goes: the accounts it is meant for,
 * and a column per folder for the accounts whose sessions there get it. One
 * that takes a system skill's name goes nowhere, and its row says so.
 */
export function SkillsGrid({
  o,
  act,
  edit,
  remove
}: {
  o: LibraryOverviewDto;
  act: (p: Promise<LibraryOverviewDto | null>) => Promise<boolean>;
  edit: (entry: LibraryEntry) => void;
  remove: (entry: LibraryEntry) => void;
}) {
  return (
    // Where the screen cannot give the window the width, the grid scrolls sooner than squeeze the names.
    <div style={{ overflowX: 'auto' }}>
      <table className="library-grid">
        <thead>
          <tr>
            <th rowSpan={o.folders.length ? 2 : 1}>Skill</th>
            <th
              rowSpan={o.folders.length ? 2 : 1}
              title="The accounts each is meant for — every account, every one of a CLI, or those you name"
            >
              Meant for
            </th>
            <th rowSpan={o.folders.length ? 2 : 1} title={AUDIENCE_TITLE}>
              For
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
                  <span
                    style={{
                      display: 'inline-flex',
                      alignItems: 'center',
                      gap: 3,
                      maxWidth: FOLDER_WIDTH - 12
                    }}
                  >
                    <Icon name="folder" size={14} />
                    <span
                      style={{
                        color: f.found ? undefined : RED,
                        overflow: 'hidden',
                        textOverflow: 'ellipsis'
                      }}
                    >
                      {baseName(f.path)}
                    </span>
                    <ToolButton
                      icon="close"
                      size={18}
                      title={`Stop giving skills to sessions in ${f.path}\nWhat waits for them goes, those copies still as Luna wrote them`}
                      onClick={() => void act(librarySetFolder(f.path, false))}
                    />
                  </span>
                </th>
              ))}
            </tr>
          )}
        </thead>
        <tbody>
          {o.skills.map(s => {
            if (o.system.some(x => x.name === s.name)) {
              return <ReservedSkillRow key={s.name} s={s} o={o} act={act} edit={edit} remove={remove} />;
            }
            const mine = o.places.filter(p => p.skill === s.name);
            return (
              <tr key={s.name}>
                <td style={{ maxWidth: 0, minWidth: 200, width: '100%' }}>
                  <NameCell name={s.name} />
                  <div
                    title={s.problem ?? s.description}
                    style={{
                      fontSize: 'var(--fs-2)',
                      color: s.problem ? RED : 'var(--faint)',
                      overflow: 'hidden',
                      textOverflow: 'ellipsis',
                      whiteSpace: 'nowrap'
                    }}
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
                <td className="cell">
                  <AudiencePicker entry={{ skill: s.name }} audience={s.audience} act={act} />
                </td>
                {o.folders.map((f, i) => (
                  <td key={f.path} className="cell">
                    <FolderCell
                      id={`lib-${s.name}-folder-${i}`}
                      skill={s.name}
                      folder={f}
                      accounts={o.accounts}
                      act={act}
                    />
                    <CellDot places={mine.filter(p => p.target.kind === 'folder' && p.target.path === f.path)} />
                  </td>
                ))}
                <td className="cell">
                  <span style={{ display: 'inline-flex', gap: 2 }}>
                    <ToolButton
                      icon="pencil"
                      size={22}
                      title={`Edit ${s.name}'s SKILL.md`}
                      onClick={() => edit({ skill: s.name })}
                    />
                    <ToolButton
                      icon="folder"
                      size={22}
                      title={`Open ${s.name}'s folder — for the files beside SKILL.md`}
                      onClick={() => void libraryOpen(s.name)}
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
          })}
        </tbody>
      </table>
    </div>
  );
}
