/**
 * Everything the UI knows about Claude Code: its settings vocabulary, where
 * defaults come from, the trust bit, the login session, how its limits and a
 * chat's flags are drawn. The Codex twin lives next door and shares nothing
 * but the shape.
 */
import type { CSSProperties, ReactNode } from 'react';
import type { Account, Chat, ClaudeChat, ClaudeSettings, Effort, ModelLabel, PermMode } from '../../../shared/types';
import {
  EFFORTS, MODELS, PERM_HINTS, effortFromSetting, modelFromSetting, permFromSetting
} from '../../../shared/types';
import {
  claudeDefaults, claudeFolderTrusted, claudeTrustFolder, ensureClaudeSession, type ClaudeDefaultSource
} from '../../../ipc/commands';
import { limitColor } from '../../../shared/lib/format';
import { Segmented } from '../../../shared/ui/Segmented';
import type { Draft, From } from '../../new-chat/draft';

export const label = 'Claude Code';
export const vendor = 'Anthropic';

/** Where a chat opens when no settings file says otherwise. */
export const STOCK: ClaudeSettings = { model: 'Opus', effort: 'high', perm: 'Bypass' };

export const SOURCE_LABELS: Record<ClaudeDefaultSource, string> = {
  account: "the account's settings.json",
  project: "the project's .claude/settings.json",
  'project-local': "the project's .claude/settings.local.json",
  managed: 'the machine-wide managed settings'
};

export const settingsNote = 'Opens on this account’s Claude Code settings; anything changed here applies to this chat only';

/**
 * Model, effort and permission mode as Claude Code itself would resolve them
 * for this account in this folder. A value Luna has no equivalent for — a
 * model it does not list, an effort the CLI has since renamed — is treated as
 * nothing said, which leaves the stock default rather than a chat that cannot
 * be described in the title bar.
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
    from.model = SOURCE_LABELS[dto.model.source];
  }
  const effort = dto?.effort ? effortFromSetting(dto.effort.value) : null;
  if (effort && dto?.effort) {
    values.effort = effort;
    from.effort = SOURCE_LABELS[dto.effort.source];
  }
  const perm = dto?.permissionMode ? permFromSetting(dto.permissionMode.value) : null;
  if (perm && dto?.permissionMode) {
    values.perm = perm;
    from.perm = SOURCE_LABELS[dto.permissionMode.source];
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

/** The chat's flags in the pane title bar. */
export function Chips({ chat, chip, softChip }: { chat: ClaudeChat; chip: CSSProperties; softChip: CSSProperties }) {
  return (
    <>
      <span title="Model" style={softChip}>{chat.model}</span>
      <span title={`Effort: ${chat.effort}`} style={softChip}>{chat.effort}</span>
      <span title={`Permission mode: ${chat.perm}`} style={chip}>{chat.perm[0]}</span>
    </>
  );
}

/** One line for the sidebar row's tooltip and the like. */
export const summary = (chat: Chat) =>
  chat.provider === 'claude' ? `${chat.model} · ${chat.effort} · ${chat.perm}` : '';

// -------------------------------------------------------------- accounts --

const LK: Array<['h5' | 'week' | 'fable', string]> = [['h5', '5 hours'], ['week', 'week'], ['fable', 'fable']];

export function LimitBars({ account }: { account: Account }) {
  const lim = account.limits;
  if (lim.kind !== 'claude') return null;
  return (
    <>
      {LK.map(([k, full]) => (
        <div key={k} style={{ display: 'flex', alignItems: 'center', gap: 5, marginBottom: 3 }}>
          <span style={{ fontSize: 'var(--fs-1)', color: 'var(--dim)', width: 34, flex: 'none', whiteSpace: 'nowrap', overflow: 'hidden' }}>{full}</span>
          <div className="xp-sunken" style={{ flex: 1, height: 6, background: 'var(--track)', overflow: 'hidden' }}>
            {account.haveUsage && (
              <div style={{ height: '100%', width: `${Math.round(lim[k] * 100)}%`, background: limitColor(lim[k]) }} />
            )}
          </div>
          <span style={{ fontSize: 'var(--fs-1)', color: 'var(--dim)', width: 30, flex: 'none', textAlign: 'right', fontVariantNumeric: 'tabular-nums' }}>
            {account.haveUsage ? `${Math.round(lim[k] * 100)}%` : '—'}
          </span>
          <span style={{ fontSize: 'var(--fs-1)', color: 'var(--faint)', width: 42, flex: 'none', textAlign: 'right', whiteSpace: 'nowrap', overflow: 'hidden', fontVariantNumeric: 'tabular-nums' }}>
            {account.haveUsage ? lim.resets[k] : ''}
          </span>
        </div>
      ))}
    </>
  );
}

/** The worst of the account's bars, for the row's dot. */
export const worstLimit = (account: Account) =>
  account.limits.kind === 'claude' ? Math.max(account.limits.h5, account.limits.week) : 0;

/** The instant the long window resets, for "resets Aug 24, 21:00". */
export const longResetAt = (account: Account) =>
  account.limits.kind === 'claude' ? account.limits.weekResetAt : null;
