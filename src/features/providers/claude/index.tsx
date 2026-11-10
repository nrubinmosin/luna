/**
 * Everything the UI knows about Claude Code: its settings vocabulary, where
 * defaults come from, the trust bit, the login session, how its limits and a
 * chat's flags are drawn. The Codex twin lives next door and shares nothing
 * but the shape.
 */
import { useEffect, useState, type CSSProperties, type ReactNode } from 'react';
import type { Account, ClaudeChat, ClaudeSettings, Effort, ModelLabel, PermMode } from '../../../shared/types';
import {
  EFFORTS,
  MODELS,
  PERM_HINTS,
  ULTRACODE,
  effortFromSetting,
  modelFromSetting,
  modelLabelOf,
  permFromSetting
} from '../../../shared/types';
import { claudeDefaults, claudeModels, type ClaudeDefaultSource, type ClaudeModelDto } from '../../../ipc/defaults';
import { ensureClaudeSession } from '../../../ipc/sessions';
import { claudeFolderTrusted, claudeTrustFolder } from '../../../ipc/trust';
import { ModelSelect } from '../../../shared/ui/ModelSelect';
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

export const settingsNote =
  'Opens on this account’s Claude Code settings; anything changed here applies to this chat only';

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
  "Ticked, Luna writes the same mark Claude Code's own trust prompt writes, and the chat opens on its prompt. " +
  'Unticked, nothing is written and Claude Code asks in the chat itself.';

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
const worktreeParentRe = /^(.*)[\\/]\.claude[\\/]worktrees[\\/][^\\/]+$/;
export const projectOf = (cwd: string) => worktreeParentRe.exec(cwd)?.[1] ?? null;
export const branchPrefix = 'worktree-';
/** Alt+V: Claude Code's image paste on Windows, and taken elsewhere too. */
export const pasteImageKey = '\x1bv';

const worktreeNameRe = /^(.*)[\\/]\.claude[\\/]worktrees[\\/]([A-Za-z0-9._-]+)[\\/]?$/;
const samePath = (a: string, b: string) =>
  a.replace(/\//g, '\\').replace(/\\+$/, '').toLowerCase() === b.replace(/\//g, '\\').replace(/\\+$/, '').toLowerCase();

/** The name `claude --worktree <name>` knows this folder's worktree by, or
 *  null when the path is not a worktree Claude Code made right in `folder`. */
export function worktreeName(folder: string, path: string | null | undefined): string | null {
  const [, root, name] = (path && worktreeNameRe.exec(path)) || [];
  return root !== undefined && name !== undefined && samePath(root, folder) ? name : null;
}

// ---------------------------------------------------------------- dialog --

/** The models listed before the account's own list has come: the four
 *  aliases, each at every level. */
const STOCK_MODELS: ClaudeModelDto[] = MODELS.map(m => ({
  slug: m.toLowerCase(),
  modelId: null,
  displayName: m,
  defaultEffort: null,
  efforts: EFFORTS.filter(e => e !== ULTRACODE),
  effortHints: {},
  more: false
}));

/** The effort buttons for a model: its own levels and `ultracode`, none for
 *  a model that takes no effort, every level for one the list does not
 *  know. */
export function effortsFor(listed: ClaudeModelDto | undefined): Effort[] {
  if (!listed) return EFFORTS;
  return listed.efforts.length ? [...listed.efforts, ULTRACODE] : [];
}

/** The effort a chat on this model must move to from `current`, or null
 *  where `current` will do: none (`''`, no `--effort`) for a model that
 *  takes none, the model's own default (else `high`, else its first level)
 *  for one that does not take `current`. */
export function effortMovedTo(listed: ClaudeModelDto | undefined, current: Effort): Effort | null {
  const takes = effortsFor(listed);
  if (!takes.length) return current === '' ? null : '';
  if (takes.includes(current)) return null;
  const own = listed?.efforts ?? EFFORTS;
  return listed?.defaultEffort ?? (own.includes('high') ? 'high' : (own[0] ?? 'high'));
}

export function Fields({
  draft,
  origin,
  accountPath
}: {
  draft: Draft<ClaudeSettings>;
  origin: (field: keyof ClaudeSettings) => ReactNode;
  accountPath: string;
}) {
  const { settings, pick } = draft;
  const labelStyle: CSSProperties = { fontSize: 'var(--fs-3)', color: 'var(--dim)', marginBottom: 4, fontWeight: 600 };
  const hintStyle: CSSProperties = { fontSize: 'var(--fs-2)', color: 'var(--faint)' };
  // The account's own model list (claude/models.rs); the four aliases until
  // it comes, so the dialog is usable at once.
  const [models, setModels] = useState<ClaudeModelDto[] | null>(null);
  useEffect(() => {
    let stale = false;
    // eslint-disable-next-line react-hooks/set-state-in-effect -- another account's list goes while this one's loads
    setModels(null);
    void claudeModels(accountPath)
      .then(m => !stale && setModels(m))
      .catch(() => !stale && setModels([]));
    return () => {
      stale = true;
    };
  }, [accountPath]);
  const list = models?.length ? models : STOCK_MODELS;

  const listed = list.find(m => modelLabelOf(m.slug) === settings.model);
  const efforts = effortsFor(listed);
  const effortItems =
    efforts.length && settings.effort && !efforts.includes(settings.effort) ? [...efforts, settings.effort] : efforts;

  // A model that does not take the level shown moves it to its own default
  // level, a hand-picked one included: a chat cannot run at a level its
  // model lacks. One that takes it keeps it, whether a settings file named
  // it, Luna's stock chose it or it was picked by hand. A model that takes
  // no effort runs with none.
  const moveTo = effortMovedTo(listed, settings.effort);
  useEffect(() => {
    if (moveTo !== null) draft.force('effort', moveTo);
    // `draft.force` is a new function every render; the model is what matters.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [listed?.slug, moveTo]);

  // Main models first, the rest under "More…"; a model a settings file names
  // that the list does not know still gets an entry of its own, chosen.
  const modelItems = list.map(m => ({
    value: modelLabelOf(m.slug),
    label: m.displayName,
    group: m.more ? 'More…' : undefined
  }));
  if (!listed) modelItems.push({ value: settings.model, label: settings.model, group: undefined });

  return (
    <>
      <div>
        <div style={labelStyle}>Model</div>
        <ModelSelect
          items={modelItems}
          value={settings.model}
          defaultValue={draft.resolved.model}
          onPick={(v: ModelLabel) => pick('model', v)}
        />
        {origin('model')}
      </div>

      <div>
        <div style={labelStyle}>Effort</div>
        {effortItems.length ? (
          <Segmented
            items={effortItems}
            value={settings.effort}
            onPick={(v: Effort) => pick('effort', v)}
            hints={listed?.effortHints}
          />
        ) : (
          <div style={hintStyle}>{listed?.displayName ?? settings.model} takes no effort level.</div>
        )}
        {effortItems.length > 0 && origin('effort')}
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
                <span
                  style={{
                    fontSize: 'var(--fs-2)',
                    color: 'var(--faint)',
                    overflow: 'hidden',
                    textOverflow: 'ellipsis',
                    whiteSpace: 'nowrap'
                  }}
                >
                  {PERM_HINTS[p]}
                </span>
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
      <span
        className="pchip"
        title={
          chat.effort ? `Model and effort: ${chat.model}, ${chat.effort}` : `Model: ${chat.model} (no effort level)`
        }
      >
        <b>{chat.model}</b> {chat.effort}
      </span>
      <span className="pchip opt-2" title={`Permission mode: ${chat.perm} — ${PERM_HINTS[chat.perm]}`}>
        {chat.perm}
      </span>
    </>
  );
}
