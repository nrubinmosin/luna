/**
 * Everything the UI knows about Claude Code: its settings vocabulary, where
 * defaults come from, the trust bit, the login session, how its limits and a
 * chat's flags are drawn. The Codex twin lives next door and shares nothing
 * but the shape.
 */
import type { CSSProperties, ReactNode } from 'react';
import type { Account, ClaudeChat, ClaudeSettings, Effort, ModelLabel, PermMode } from '../../../shared/types';
import {
  EFFORTS, MODELS, PERM_HINTS, effortFromSetting, modelFromSetting, permFromSetting
} from '../../../shared/types';
import {
  claudeDefaults, claudeFolderTrusted, claudeTrustFolder, ensureClaudeSession, type ClaudeDefaultSource
} from '../../../ipc/commands';
import { Segmented } from '../../../shared/ui/Segmented';
import type { Draft, From } from '../../new-chat/draft';

export const label = 'Claude Code';

/** Where a chat opens when no settings file says otherwise: the core's stock
 *  (claude/defaults.rs), for the moment before it answers or if it cannot. */
export const STOCK: ClaudeSettings = { model: 'Opus', effort: 'high', perm: 'Bypass' };

export const SOURCE_LABELS: Record<Exclude<ClaudeDefaultSource, 'stock'>, string> = {
  account: "the account's settings.json",
  project: "the project's .claude/settings.json",
  'project-local': "the project's .claude/settings.local.json",
  managed: 'the machine-wide managed settings'
};

export const settingsNote = 'Opens on this account’s Claude Code settings; anything changed here applies to this chat only';

/**
 * Model, effort and permission mode as Claude Code itself would resolve them
 * for this account in this folder, settled by the core: a value Luna has no
 * equivalent for — a model it does not list, an effort the CLI has since
 * renamed — comes back as the stock default rather than a chat that cannot
 * be described in the title bar. A stock value names no file to come from.
 */
export async function settingsDefaults(
  accountPath: string,
  folder: string
): Promise<{ values: ClaudeSettings; from: From<ClaudeSettings> }> {
  const dto = await claudeDefaults(accountPath, folder).catch(() => null);
  const values: ClaudeSettings = { ...STOCK };
  const from: From<ClaudeSettings> = {};

  const model = dto?.model ? modelFromSetting(dto.model.value) : null;
  if (model && dto?.model) {
    values.model = model;
    if (dto.model.source !== 'stock') from.model = SOURCE_LABELS[dto.model.source];
  }
  const effort = dto?.effort ? effortFromSetting(dto.effort.value) : null;
  if (effort && dto?.effort) {
    values.effort = effort;
    if (dto.effort.source !== 'stock') from.effort = SOURCE_LABELS[dto.effort.source];
  }
  const perm = dto?.permissionMode ? permFromSetting(dto.permissionMode.value) : null;
  if (perm && dto?.permissionMode) {
    values.perm = perm;
    if (dto.permissionMode.source !== 'stock') from.perm = SOURCE_LABELS[dto.permissionMode.source];
  }
  return { values, from };
}

/** How a value reads in "back to …". */
export const describe = (_field: keyof ClaudeSettings, value: ClaudeSettings[keyof ClaudeSettings]) => String(value);

// ----------------------------------------------------------------- trust --

export const folderTrusted = claudeFolderTrusted;
export const trustFolder = claudeTrustFolder;
export const trustNote =
  "Creating the chat marks it trusted for this account — the same thing Claude Code's trust prompt does. " +
  "The prompt can't be shown under an isolated worktree, which is why it otherwise asks you to open the folder without isolation first.";

// ----------------------------------------------------------------- login --

/** Bare `claude` session inside the account's config dir: on a fresh folder
 *  it walks through the login flow and drops credentials there; on an
 *  existing one the user can run /login to re-authenticate. */
export const loginSession = (id: string, account: Account) =>
  ensureClaudeSession({
    chatId: id,
    folder: account.path,
    accountPath: account.path,
    model: 'Sonnet',
    effort: 'medium',
    perm: 'Ask',
    worktree: false
  });

// -------------------------------------------------------------- worktree --

/** `<repo>/.claude/worktrees/<name>` is a session's worktree, not its project. */
export const worktreeRe = /[\\/]\.claude[\\/]worktrees[\\/]/i;
export const worktreeParentRe = /^(.*)[\\/]\.claude[\\/]worktrees[\\/][^\\/]+$/;
export const branchPrefix = 'worktree-';

const worktreeNameRe = /^(.*)[\\/]\.claude[\\/]worktrees[\\/]([A-Za-z0-9._-]+)[\\/]?$/;
const samePath = (a: string, b: string) =>
  a.replace(/\//g, '\\').replace(/\\+$/, '').toLowerCase() === b.replace(/\//g, '\\').replace(/\\+$/, '').toLowerCase();

/** The name `claude --worktree <name>` knows this folder's worktree by, or
 *  null when the path is not a worktree Claude Code made right in `folder`. */
export function worktreeName(folder: string, path: string | null | undefined): string | null {
  const m = path ? worktreeNameRe.exec(path) : null;
  return m && samePath(m[1], folder) ? m[2] : null;
}

// ---------------------------------------------------------------- dialog --

export function Fields({ draft, origin }: {
  draft: Draft<ClaudeSettings>;
  origin: (field: keyof ClaudeSettings) => ReactNode;
}) {
  const { settings, pick } = draft;
  const labelStyle: CSSProperties = { fontSize: 'var(--fs-3)', color: 'var(--dim)', marginBottom: 4, fontWeight: 600 };
  return (
    <>
      <div>
        <div style={labelStyle}>Model</div>
        <Segmented items={MODELS} value={settings.model} onPick={(v: ModelLabel) => pick('model', v)} />
        {origin('model')}
      </div>

      <div>
        <div style={labelStyle}>Effort</div>
        <Segmented items={EFFORTS} value={settings.effort} onPick={(v: Effort) => pick('effort', v)} />
        {origin('effort')}
      </div>

      <div>
        <div style={labelStyle}>Permission mode</div>
        <div style={{ display: 'flex', flexDirection: 'column', gap: 5 }}>
          {(Object.keys(PERM_HINTS) as PermMode[]).map(p => {
            const on = settings.perm === p;
            const id = `perm-${p}`;
            return (
              <div key={p} className="field-row">
                <input type="radio" id={id} name="perm-mode" checked={on} onChange={() => pick('perm', p)} />
                <label htmlFor={id} style={{ fontWeight: 600, whiteSpace: 'nowrap', cursor: 'default' }}>
                  {p}
                </label>
                <span style={{ fontSize: 'var(--fs-2)', color: 'var(--faint)', overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{PERM_HINTS[p]}</span>
              </div>
            );
          })}
        </div>
        {origin('perm')}
      </div>
    </>
  );
}

// ------------------------------------------------------------------ pane --

/** The chat's settings as chips on the pane title bar. */
export function Meta({ chat }: { chat: ClaudeChat }) {
  return (
    <>
      <span className="pchip" title={`Model and effort: ${chat.model}, ${chat.effort}`}>
        <b>{chat.model}</b> {chat.effort}
      </span>
      <span className="pchip opt-2" title={`Permission mode: ${chat.perm} — ${PERM_HINTS[chat.perm]}`}>{chat.perm}</span>
    </>
  );
}
