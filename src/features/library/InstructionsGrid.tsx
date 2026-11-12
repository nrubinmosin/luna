import { useState } from 'react';
import {
  libraryRenameInstructions,
  librarySetFolderInstructions,
  librarySetInstructions,
  type LibraryFolderDto,
  type LibraryInstructionsDto,
  type LibraryOverviewDto
} from '../../ipc/library';
import { Icon, ToolButton } from '../../shared/ui/icons';
import { PROVIDERS, PROVIDER_LABEL } from '../../shared/types';
import { AUDIENCE_TITLE, AudiencePicker } from './AudiencePicker';
import { CellDot, NameCell, Tick } from './cells';
import { baseName } from './paths';
import { accountKey, how, type AccountRef } from './places';
import { RED } from './presentation';
import { ReachPicker } from './ReachPicker';

/** A select's value for none at all, which no instructions' name can be (library/skills.rs `check_name`). */
const NO_INSTRUCTIONS = ' none';

/** A folder's entry that decides for an account, as library/folders.rs `folder_instructions` picks it — its own, its
 *  CLI's, all — with its key. */
function folderEntry(
  instructions: Record<string, string | null> | undefined,
  a: AccountRef
): { key: string; choice: string | null } | undefined {
  for (const key of [accountKey(a), a.provider, 'all'])
    if (instructions && Object.hasOwn(instructions, key)) return { key, choice: instructions[key] ?? null };
  return undefined;
}

/**
 * What the sessions on an account in a folder get in place of its
 * instructions: what holds outside the folder, none, or other instructions.
 * Codex reads its account's AGENTS.md in every session and has no way to
 * leave it out of one, so its accounts' boxes stay as they are.
 */
function FolderInstructions({
  folder,
  account,
  names,
  act
}: {
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
        void act(
          librarySetFolderInstructions(
            folder.path,
            accountKey(account),
            v === '',
            v === '' || v === NO_INSTRUCTIONS ? null : v
          )
        );
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
export function InstructionsGrid({
  o,
  busy,
  act,
  edit,
  remove
}: {
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
    o.places.filter(
      p =>
        p.instructions === i &&
        p.target.kind === 'account' &&
        p.target.provider === a.provider &&
        p.target.name === a.name
    );

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
          style={{
            fontSize: 'var(--fs-2)',
            color: i.problem ? 'var(--wait)' : 'var(--faint)',
            overflow: 'hidden',
            textOverflow: 'ellipsis',
            whiteSpace: 'nowrap'
          }}
        >
          {i.problem ?? i.description}
        </div>
      </td>
      <td className="cell">
        <ReachPicker
          id={`lib-ins-${i.name}`}
          reach={i.accounts}
          accounts={o.accounts}
          onChange={r => act(librarySetInstructions(i.name, r ?? []))}
        />
      </td>
      <td className="cell">
        <AudiencePicker entry={{ instructions: i.name }} audience={i.audience} act={act} />
      </td>
      {cols.map(a => {
        const key = accountKey(a);
        const way = how(i.accounts, a);
        // For some roles only: no account's file, so no radio among the
        // others; a box for whether its sessions on the account get it.
        if (i.perSession) {
          const named = i.accounts.includes(key);
          return (
            <td key={key} className="cell">
              <Tick
                id={`lib-ins-${i.name}-${key}`}
                checked={!!way}
                disabled={busy || (!!way && !named)}
                title={
                  way
                    ? `Its sessions on ${a.name} get ${i.name} as Luna starts them, ${way}${named ? '\nClick to keep it from them' : '\nThe reach picker takes it back'}`
                    : `Give its sessions on ${a.name} ${i.name} as Luna starts them`
                }
                onChange={on =>
                  void act(
                    librarySetInstructions(i.name, on ? [...i.accounts, key] : i.accounts.filter(r => r !== key))
                  )
                }
              />
            </td>
          );
        }
        const gets = o.instructionsFor[key] === i.name;
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
          <ToolButton
            icon="trash"
            size={22}
            danger
            title={`Take ${i.name} out of the library — asks first`}
            onClick={() => remove(i.name)}
          />
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
          <th rowSpan={2} title={AUDIENCE_TITLE}>
            For
          </th>
          {PROVIDERS.map(provider => {
            const span = cols.filter(a => a.provider === provider).length;
            return span ? (
              <th
                key={provider}
                colSpan={span}
                className="group"
                title={`Which instructions each ${PROVIDER_LABEL[provider]} account gets — one at most`}
              >
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
              style={{
                fontSize: 'var(--fs-2)',
                color: 'var(--faint)',
                overflow: 'hidden',
                textOverflow: 'ellipsis',
                whiteSpace: 'nowrap'
              }}
            >
              Nothing from the library; the account’s own file, if any, is left alone
            </div>
          </td>
          <td />
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
                <span
                  style={{
                    overflow: 'hidden',
                    textOverflow: 'ellipsis',
                    whiteSpace: 'nowrap',
                    color: f.found ? undefined : RED
                  }}
                >
                  {baseName(f.path)}
                </span>
              </div>
              <div
                style={{
                  fontSize: 'var(--fs-2)',
                  color: 'var(--faint)',
                  overflow: 'hidden',
                  textOverflow: 'ellipsis',
                  whiteSpace: 'nowrap'
                }}
              >
                The sessions Luna starts there, in place of the account’s
              </div>
            </td>
            <td />
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
