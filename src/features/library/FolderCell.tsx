import { librarySetFolderSkill, type LibraryFolderDto, type LibraryOverviewDto } from '../../ipc/library';
import { PROVIDER_LABEL, type Provider } from '../../shared/types';
import { Tick } from './cells';
import { accountKey, how, type AccountRef } from './places';
import { ReachPicker } from './ReachPicker';

/**
 * Which accounts' sessions in a folder get a skill: by a reach of the
 * folder's own — a tick where every account gets it there, a dash where some
 * do, a cross where none does — or, as the folder says nothing of it, by what
 * they get anyway, drawn faint. A click opens the accounts to pick from.
 */
export function FolderCell({
  id,
  skill,
  folder,
  accounts,
  act
}: {
  id: string;
  skill: string;
  folder: LibraryFolderDto;
  accounts: AccountRef[];
  act: (p: Promise<LibraryOverviewDto>) => Promise<boolean>;
}) {
  // Own keys only: a skill may be named `constructor`.
  const own = Object.hasOwn(folder.skills, skill) ? (folder.skills[skill] ?? null) : null;
  const inherited = Object.hasOwn(folder.inherited, skill) ? (folder.inherited[skill] ?? []) : [];
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
