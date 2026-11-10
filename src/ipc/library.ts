import { call } from './core';
import type { Provider } from '../shared/types';
import type { SystemSkillState } from './applied';

/** An account, or what the sessions in a folder get on one account. */
export type LibraryTarget =
  | { kind: 'account'; provider: Provider; name: string }
  | { kind: 'folder'; path: string; provider: Provider; name: string };
export type PlaceStatus = 'current' | 'behind' | 'missing' | 'changed' | 'gone' | 'taken' | 'skipped' | 'failed';

/** One copy the library wants somewhere, and where it stands. */
export interface PlaceDto {
  dest: string;
  target: LibraryTarget;
  /** Null for an instructions file. */
  skill: string | null;
  /** Which instructions, for an instructions file. */
  instructions: string | null;
  /** The name a folder's copy goes by, where the skill's own is another skill's there. */
  shownAs: string | null;
  status: PlaceStatus;
  /** Changed there, and in the library too, since Luna wrote it. */
  both: boolean;
  problem: string | null;
}
/** Which accounts something goes into: `all`, a provider's every account
 *  (`claude`, `codex`) or one account (`provider/name`) per entry. */
export type Reach = string[];
/** Whose sessions a skill or instructions file is for: the person's own chats, the helpers other sessions
 *  start, or both. */
export type Audience = 'both' | 'chats' | 'helpers';
export interface LibrarySkillDto {
  name: string;
  description: string;
  /** Why it is laid out nowhere, while it is not fit to be. */
  problem: string | null;
  /** The accounts it goes into, wherever no folder says otherwise. */
  accounts: Reach;
  audience: Audience;
}
export interface LibraryInstructionsDto {
  name: string;
  /** Its first heading, `#` or else `##`; empty without one. */
  description: string;
  /** Why it is laid out nowhere: blank, or a file that will not do. */
  problem: string | null;
  /** The accounts its reach takes in; a closer reach of other instructions wins. */
  accounts: Reach;
  audience: Audience;
}
export interface LibraryFolderDto {
  path: string;
  /** The accounts whose sessions here get a skill, by the skills it says anything of; empty: none. */
  skills: Record<string, Reach>;
  /** The instructions its sessions get in place of their account's, by the reach entry they are for (as a
   *  rule `provider/name`); null: none at all. Absent: it goes by what holds outside it. */
  instructions?: Record<string, string | null>;
  found: boolean;
  /** The accounts, as `provider/name`, whose sessions here would get each skill were the folder to say
   *  nothing of it; absent: none. */
  inherited: Record<string, string[]>;
}
/** A copy of Luna's no longer wanted where it lies, left there because it was changed since Luna wrote it. */
export interface KeptCopyDto {
  dest: string;
  /** Whose it was; null where its path no longer says. */
  target: LibraryTarget | null;
  /** The skill's name there; null for an instructions file. */
  skill: string | null;
}
export interface LibraryOverviewDto {
  root: string;
  isDefault: boolean;
  /** False while the library's folder is not there. */
  found: boolean;
  instructions: LibraryInstructionsDto[];
  /** Accounts that get no instructions however others reach them. */
  noInstructions: Reach;
  /** The instructions each account gets, by `provider/name`; absent: none. */
  instructionsFor: Record<string, string>;
  /** The instructions.md of before there could be several, left beside instructions/main.md. */
  oldInstructions: string | null;
  skills: LibrarySkillDto[];
  /** Every account, to pick a reach from. */
  accounts: { provider: Provider; name: string }[];
  folders: LibraryFolderDto[];
  places: PlaceDto[];
  /** Accounts' instruction files Luna did not write, where no instructions go, to start new ones from. */
  foundInstructions: { dest: string; target: LibraryTarget }[];
  /** Codex accounts whose AGENTS.override.md hides the AGENTS.md Luna writes. */
  overridden: string[];
  /** Copies no longer wanted, left in place because they had changed since Luna wrote them: each until it is
   *  taken out or kept. */
  kept: KeptCopyDto[];
  /** The system skills as each is served now: Luna's own text, your override of it, or an override that will
   *  not do (library/system.rs). */
  system: SystemSkillState[];
  /** Folders set aside in the library's `system/`, read by nothing: a former override that a copy into the
   *  backups on another volume did not take whole, or one an adoption replaced and could not move. */
  systemAside: string[];
  /** From a put-back or a kept copy's settling only: the places it could not settle, each with why. */
  unsettled?: string[];
}
export interface LibraryFileDto {
  path: string;
  text: string;
  exists: boolean;
}
/** One file of a copy as Luna wrote it last, as the library has it and as it is there. */
export interface FileDiffDto {
  path: string;
  base: string | null;
  library: string | null;
  there: string | null;
  binary: boolean;
}

export const libraryOverview = () => call<LibraryOverviewDto | null>('library_overview', {}, null);
/** Lays the library out — everything that needs no asking. */
export const librarySync = () => call<LibraryOverviewDto | null>('library_sync', {}, null);
export const librarySetSource = (path: string) => call<LibraryOverviewDto>('library_set_source', { path });
/** `name` null: the accounts that get no instructions. Each entry is taken from the instructions that had it. */
export const librarySetInstructions = (name: string | null, accounts: Reach) =>
  call<LibraryOverviewDto>('library_set_instructions', { name, accounts });
export const librarySetAccounts = (skill: string, accounts: Reach) =>
  call<LibraryOverviewDto>('library_set_accounts', { skill, accounts });
/** Lists a folder, saying nothing of any skill yet, or takes it off the list with all it says. */
export const librarySetFolder = (path: string, listed: boolean) =>
  call<LibraryOverviewDto>('library_set_folder', { path, listed });
/** The accounts whose sessions in a folder get a skill (empty: none), or null: the folder says nothing of it. */
export const librarySetFolderSkill = (path: string, skill: string, accounts: Reach | null) =>
  call<LibraryOverviewDto>('library_set_folder_skill', { path, skill, accounts });
/** What sessions on an account in a folder get in place of its instructions: `name`'s or none (null);
 *  `inherit`: what holds outside the folder. */
export const librarySetFolderInstructions = (path: string, account: string, inherit: boolean, name: string | null) =>
  call<LibraryOverviewDto>('library_set_folder_instructions', { path, account, inherit, name });
export const librarySkip = (dest: string, on: boolean) => call<LibraryOverviewDto>('library_skip', { dest, on });
/** `take`: the copy there becomes the library's; otherwise the library's goes back there. */
export const libraryResolve = (dest: string, take: boolean) =>
  call<LibraryOverviewDto>('library_resolve', { dest, take });
/** The library's copy back at each of `dests`, then a pass; places left out stay out. Over what is there only at
 *  the places in `replace`, elsewhere only where nothing is. */
export const libraryPutBack = (dests: string[], replace: string[]) =>
  call<LibraryOverviewDto>('library_put_back', { dests, replace });
/** Settles copies left in place as no longer wanted: `takeOut` backs each up and removes it, otherwise Luna
 *  forgets it and it stays as yours. */
export const libraryKept = (dests: string[], takeOut: boolean) =>
  call<LibraryOverviewDto>('library_kept', { dests, takeOut });
export const libraryDiff = (dest: string) => call<FileDiffDto[]>('library_diff', { dest }, []);
/** A skill's SKILL.md or instructions, by name. */
export type LibraryEntry = { skill: string } | { instructions: string };
/** The file the editor has open: a library entry, or a system skill's override (`exists` false: none yet, and
 *  the text Luna's own to start from). */
export type EditorEntry = LibraryEntry | { system: string };
/** Whose sessions a skill or instructions file is for. */
export const librarySetAudience = (entry: LibraryEntry, audience: Audience) =>
  call<LibraryOverviewDto>('library_set_audience', { entry, audience });
export const libraryRead = (entry: EditorEntry) => call<LibraryFileDto>('library_read', { entry });
export const libraryWrite = (entry: EditorEntry, text: string) =>
  call<LibraryOverviewDto>('library_write', { entry, text });
export const libraryCheckSkill = (skill: string, text: string) =>
  call<string | null>('library_check_skill', { skill, text }, null);
/** Why this text would not do as a system skill's override (Luna serves its own in its place), or null. */
export const libraryCheckSystem = (name: string, text: string) =>
  call<string | null>('library_check_system', { name, text }, null);
/** Luna's own text of a system skill beside your override of it (null: none). */
export interface SystemCompareDto {
  luna: string;
  yours: string | null;
}
export const librarySystemCompare = (name: string) => call<SystemCompareDto>('library_system_compare', { name });
/** Back to Luna's own: the override goes to Luna's backups. */
export const librarySystemReset = (name: string) => call<LibraryOverviewDto>('library_system_reset', { name });
/** A library skill of a system skill's name becomes its override, its folder moved there. */
export const librarySystemAdopt = (skill: string) => call<LibraryOverviewDto>('library_system_adopt', { skill });
export const libraryNewSkill = (name: string) => call<LibraryOverviewDto>('library_new_skill', { name });
export const libraryDeleteSkill = (name: string) => call<LibraryOverviewDto>('library_delete_skill', { name });
export const libraryNewInstructions = (name: string) => call<LibraryOverviewDto>('library_new_instructions', { name });
export const libraryRenameInstructions = (from: string, to: string) =>
  call<LibraryOverviewDto>('library_rename_instructions', { from, to });
export const libraryDeleteInstructions = (name: string) =>
  call<LibraryOverviewDto>('library_delete_instructions', { name });
/** The library's folder, or a skill's, in the file manager. */
export const libraryOpen = (skill: string | null) => call<void>('library_open', { skill });
