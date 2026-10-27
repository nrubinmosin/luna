/**
 * Everything the UI knows about Codex, in Codex's own words: model as a free
 * string, reasoning effort, approval policy and sandbox mode; defaults out of
 * `config.toml`; trust as a `[projects]` entry; `codex login`; rate-limit
 * windows as the usage endpoint names them.
 */
import { useEffect, useState, type CSSProperties, type ReactNode } from 'react';
import type { Account, CodexApproval, CodexChat, CodexEffort, CodexSandbox, CodexSettings } from '../../../shared/types';
import {
  CODEX_APPROVAL_HINTS, CODEX_APPROVALS, CODEX_EFFORTS, CODEX_SANDBOX_HINTS, CODEX_SANDBOXES,
  codexApprovalFromSetting, codexEffortFromSetting, codexSandboxFromSetting
} from '../../../shared/types';
import {
  codexDefaults, codexFolderTrusted, codexModels, codexTrustFolder, ensureCodexSession,
  type CodexDefaultSource, type CodexModelDto
} from '../../../ipc/commands';
import { ModelSelect } from '../../../shared/ui/ModelSelect';
import { Segmented } from '../../../shared/ui/Segmented';
import type { Draft, From } from '../../new-chat/draft';

export const label = 'Codex';

/** Where a chat opens when config.toml says nothing: Codex's own default
 *  model (named by the core once it has the model list), and the same "just
 *  do it" stance the Claude side opens on. The core's stock
 *  (codex/defaults.rs), for the moment before it answers or if it cannot. */
export const STOCK: CodexSettings = { model: null, effort: 'high', approval: 'never', sandbox: 'danger-full-access' };

export const SOURCE_LABELS: Record<Exclude<CodexDefaultSource, 'stock'>, string> = {
  account: "the account's config.toml",
  project: "the project's .codex/config.toml",
  codex: "Codex's own default"
};

export const settingsNote = 'Opens on this account’s Codex config.toml; anything changed here applies to this chat only';

/** A chat that runs without `--model`: older chats, and any whose account
 *  had no model list to name one from. */
export const DEFAULT_MODEL_LABEL = 'Codex default';

/**
 * Model, effort, approval and sandbox as Codex would resolve them for this
 * account in this folder. A model is any string; the three modes have to be
 * ones Luna can put in a title bar, or they read as nothing said.
 */
export async function settingsDefaults(
  accountPath: string,
  folder: string
): Promise<{ values: CodexSettings; from: From<CodexSettings> }> {
  const dto = await codexDefaults(accountPath, folder).catch(() => null);
  const values: CodexSettings = { ...STOCK };
  const from: From<CodexSettings> = {};

  if (dto?.model?.value) {
    values.model = dto.model.value;
    if (dto.model.source !== 'stock') from.model = SOURCE_LABELS[dto.model.source];
  }
  const effort = dto?.effort ? codexEffortFromSetting(dto.effort.value) : null;
  if (effort && dto?.effort) {
    values.effort = effort;
    if (dto.effort.source !== 'stock') from.effort = SOURCE_LABELS[dto.effort.source];
  }
  const approval = dto?.approval ? codexApprovalFromSetting(dto.approval.value) : null;
  if (approval && dto?.approval) {
    values.approval = approval;
    if (dto.approval.source !== 'stock') from.approval = SOURCE_LABELS[dto.approval.source];
  }
  const sandbox = dto?.sandbox ? codexSandboxFromSetting(dto.sandbox.value) : null;
  if (sandbox && dto?.sandbox) {
    values.sandbox = sandbox;
    if (dto.sandbox.source !== 'stock') from.sandbox = SOURCE_LABELS[dto.sandbox.source];
  }
  return { values, from };
}

export const describe = (field: keyof CodexSettings, value: CodexSettings[keyof CodexSettings]) =>
  field === 'model' ? (value ? String(value) : DEFAULT_MODEL_LABEL) : String(value);

// ----------------------------------------------------------------- trust --

export const folderTrusted = codexFolderTrusted;
export const trustFolder = codexTrustFolder;
export const trustNote =
  "Creating the chat marks it trusted for this account in config.toml — the same entry Codex's own trust screen writes, " +
  'so the session opens straight on the prompt (and skips the Windows sandbox setup question that follows it).';

// ----------------------------------------------------------------- login --

/** `codex login` with CODEX_HOME at the account folder: the browser flow
 *  lands `auth.json` there. */
export const loginSession = (id: string, account: Account) =>
  ensureCodexSession({
    chatId: id,
    folder: account.path,
    accountPath: account.path,
    model: null,
    effort: 'medium',
    approval: 'on-request',
    sandbox: 'read-only',
    login: true
  });

// -------------------------------------------------------------- worktree --

/** `<CODEX_HOME>/worktrees/<4 hex>/<repo>` — the checkout `codex --worktree`
 *  made, on a detached HEAD. */
export const worktreeRe = /[\\/]worktrees[\\/][0-9a-f]{4}[\\/][^\\/]+[\\/]?$/i;
/** The path names the account, not the project: only git knows that. */
export const projectOf = (): string | null => null;
export const branchPrefix = null;
/** Ctrl+V: Codex reads the image off the clipboard itself. */
export const pasteImageKey = '\x16';

// ---------------------------------------------------------------- dialog --

export function Fields({ draft, origin, accountPath }: {
  draft: Draft<CodexSettings>;
  origin: (field: keyof CodexSettings) => ReactNode;
  accountPath: string;
}) {
  const { settings, pick } = draft;
  const labelStyle: CSSProperties = { fontSize: 'var(--fs-3)', color: 'var(--dim)', marginBottom: 4, fontWeight: 600 };
  const hintStyle: CSSProperties = { fontSize: 'var(--fs-2)', color: 'var(--faint)', overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' };
  // Codex's own model list: its cache file, another account's, or the CLI
  // asked directly (codex/models.rs) — never a box to type a slug into.
  const [models, setModels] = useState<CodexModelDto[] | null>(null);
  useEffect(() => {
    let stale = false;
    setModels(null);
    void codexModels(accountPath).then(m => !stale && setModels(m)).catch(() => !stale && setModels([]));
    return () => {
      stale = true;
    };
  }, [accountPath]);
  const list = models ?? [];

  // The levels the chosen model takes. A model from a settings file that the
  // list does not know keeps the stock ladder.
  const listed = list.find(m => m.slug === settings.model);
  const efforts = listed?.efforts.length ? listed.efforts : CODEX_EFFORTS;
  const effortItems = efforts.includes(settings.effort) ? efforts : [...efforts, settings.effort];

  // The effort follows the model to its own default level, unless a settings
  // file named a level or it was picked by hand.
  const effortFromFile = !!draft.from.effort && draft.from.effort !== SOURCE_LABELS.codex;
  useEffect(() => {
    if (listed?.defaultEffort && !effortFromFile) draft.follow('effort', listed.defaultEffort);
    // `draft.follow` is a new function every render; the model is what matters.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [listed?.slug, effortFromFile]);

  // A model a settings file names that the list does not know still gets an
  // entry of its own, chosen.
  const modelItems = list.map(m => ({ value: m.slug, label: m.displayName }));
  if (settings.model && !listed) modelItems.push({ value: settings.model, label: settings.model });

  return (
    <>
      <div>
        <div style={labelStyle}>Model</div>
        {modelItems.length > 0 ? (
          <ModelSelect
            items={modelItems}
            value={settings.model ?? modelItems[0].value}
            defaultValue={draft.resolved.model}
            onPick={slug => pick('model', slug)}
          />
        ) : (
          <div style={hintStyle}>
            {models === null ? 'Asking Codex for its models…' : 'Codex gave no model list — the chat runs on its default model.'}
          </div>
        )}
        {origin('model')}
      </div>

      <div>
        <div style={labelStyle}>Reasoning effort</div>
        <Segmented
          items={effortItems}
          value={settings.effort}
          onPick={(v: CodexEffort) => pick('effort', v)}
          hints={listed?.effortHints}
        />
        {origin('effort')}
      </div>

      <div>
        <div style={labelStyle}>Approval &amp; sandbox</div>
        <div style={{ display: 'flex', gap: 12 }}>
          <div style={{ flex: 1, minWidth: 0, display: 'flex', flexDirection: 'column', gap: 5 }}>
            {CODEX_APPROVALS.map(a => {
              const id = `codex-approval-${a}`;
              return (
                <div key={a} className="field-row" title={CODEX_APPROVAL_HINTS[a]}>
                  <input type="radio" id={id} name="codex-approval" checked={settings.approval === a} onChange={() => pick('approval', a as CodexApproval)} />
                  <label htmlFor={id} style={{ fontWeight: 600, whiteSpace: 'nowrap', cursor: 'default' }}>{a}</label>
                </div>
              );
            })}
            {origin('approval')}
          </div>
          <div style={{ flex: 1.4, minWidth: 0, display: 'flex', flexDirection: 'column', gap: 5 }}>
            {CODEX_SANDBOXES.map(s => {
              const id = `codex-sandbox-${s}`;
              return (
                <div key={s} className="field-row" title={CODEX_SANDBOX_HINTS[s]}>
                  <input type="radio" id={id} name="codex-sandbox" checked={settings.sandbox === s} onChange={() => pick('sandbox', s as CodexSandbox)} />
                  <label htmlFor={id} style={{ fontWeight: 600, whiteSpace: 'nowrap', cursor: 'default' }}>{s}</label>
                </div>
              );
            })}
            {origin('sandbox')}
          </div>
        </div>
        <div style={{ ...hintStyle, marginTop: 4, whiteSpace: 'normal' }}>
          {settings.approval === 'never' && settings.sandbox === 'danger-full-access'
            ? 'Runs with --dangerously-bypass-approvals-and-sandbox.'
            : `${CODEX_APPROVAL_HINTS[settings.approval]}; ${CODEX_SANDBOX_HINTS[settings.sandbox]}.`}
        </div>
      </div>
    </>
  );
}

// ------------------------------------------------------------------ pane --

export function Meta({ chat }: { chat: CodexChat }) {
  const model = chat.model ?? chat.modelSeen ?? null;
  const bypass = chat.approval === 'never' && chat.sandbox === 'danger-full-access';
  return (
    <>
      <span
        className="pchip"
        title={(chat.model ? `Model: ${chat.model}` : `Model: ${DEFAULT_MODEL_LABEL}${chat.modelSeen ? ` (running ${chat.modelSeen})` : ''}`) + `\nReasoning effort: ${chat.effort}`}
      >
        <b>{model ?? 'default'}</b> {chat.effort}
      </span>
      {bypass ? (
        <span className="pchip opt-2" title="Approval: never · Sandbox: danger-full-access (--dangerously-bypass-approvals-and-sandbox)">bypass</span>
      ) : (
        <span
          className="pchip opt-2"
          title={`Approval: ${chat.approval} — ${CODEX_APPROVAL_HINTS[chat.approval]}\nSandbox: ${chat.sandbox} — ${CODEX_SANDBOX_HINTS[chat.sandbox]}`}
        >
          {chat.approval} · {chat.sandbox}
        </span>
      )}
    </>
  );
}
