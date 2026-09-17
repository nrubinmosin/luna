/**
 * Everything the UI knows about Codex, in Codex's own words: model as a free
 * string, reasoning effort, approval policy and sandbox mode; defaults out of
 * `config.toml`; trust as a `[projects]` entry; `codex login`; rate-limit
 * windows as the usage endpoint names them.
 */
import { useEffect, useState, type CSSProperties, type ReactNode } from 'react';
import type { Account, Chat, CodexApproval, CodexChat, CodexEffort, CodexSandbox, CodexSettings } from '../../../shared/types';
import {
  CODEX_APPROVAL_HINTS, CODEX_APPROVALS, CODEX_EFFORTS, CODEX_PRESETS, CODEX_SANDBOX_HINTS, CODEX_SANDBOXES,
  codexApprovalFromSetting, codexEffortFromSetting, codexSandboxFromSetting
} from '../../../shared/types';
import {
  codexDefaults, codexFolderTrusted, codexTrustFolder, ensureCodexSession, type CodexDefaultSource
} from '../../../ipc/commands';
import { limitColor } from '../../../shared/lib/format';
import { Segmented } from '../../../shared/ui/Segmented';
import type { Draft, From } from '../../new-chat/draft';

export const label = 'Codex';
export const vendor = 'OpenAI';

/** Where a chat opens when config.toml says nothing: Codex's own default
 *  model, and the same "just do it" stance the Claude side opens on. */
export const STOCK: CodexSettings = { model: null, effort: 'high', approval: 'never', sandbox: 'danger-full-access' };

export const SOURCE_LABELS: Record<CodexDefaultSource, string> = {
  account: "the account's config.toml",
  project: "the project's .codex/config.toml"
};

export const settingsNote = 'Opens on this account’s Codex config.toml; anything changed here applies to this chat only';

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
    from.model = SOURCE_LABELS[dto.model.source];
  }
  const effort = dto?.effort ? codexEffortFromSetting(dto.effort.value) : null;
  if (effort && dto?.effort) {
    values.effort = effort;
    from.effort = SOURCE_LABELS[dto.effort.source];
  }
  const approval = dto?.approval ? codexApprovalFromSetting(dto.approval.value) : null;
  if (approval && dto?.approval) {
    values.approval = approval;
    from.approval = SOURCE_LABELS[dto.approval.source];
  }
  const sandbox = dto?.sandbox ? codexSandboxFromSetting(dto.sandbox.value) : null;
  if (sandbox && dto?.sandbox) {
    values.sandbox = sandbox;
    from.sandbox = SOURCE_LABELS[dto.sandbox.source];
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

/** `<repo>/.codex/worktrees/codex-xxxxxx` — the checkout Luna made for the chat. */
export const worktreeRe = /[\\/]\.codex[\\/]worktrees[\\/]/i;
export const worktreeParentRe = /^(.*)[\\/]\.codex[\\/]worktrees[\\/][^\\/]+$/;
export const branchPrefix = 'codex-';

// ---------------------------------------------------------------- dialog --

/** Models typed into earlier chats, newest first — the datalist behind the
 *  model box, since Codex's own list is not something Luna can ask for. */
const RECENT_KEY = 'luna.codex.models';
export const recentModels = (): string[] => {
  try {
    const raw = JSON.parse(localStorage.getItem(RECENT_KEY) ?? '[]');
    return Array.isArray(raw) ? raw.filter((s): s is string => typeof s === 'string') : [];
  } catch {
    return [];
  }
};
export const rememberModel = (model: string | null) => {
  if (!model) return;
  const next = [model, ...recentModels().filter(m => m !== model)].slice(0, 8);
  try {
    localStorage.setItem(RECENT_KEY, JSON.stringify(next));
  } catch {
    // a full or blocked localStorage only loses the suggestion list
  }
};

export function Fields({ draft, origin }: {
  draft: Draft<CodexSettings>;
  origin: (field: keyof CodexSettings) => ReactNode;
}) {
  const { settings, pick } = draft;
  const labelStyle: CSSProperties = { fontSize: 'var(--fs-3)', color: 'var(--dim)', marginBottom: 4, fontWeight: 600 };
  const hintStyle: CSSProperties = { fontSize: 'var(--fs-2)', color: 'var(--faint)', overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' };
  const [recent] = useState(recentModels);
  // The box edits a local copy so a half-typed name is not a picked one:
  // `pick` marks the field touched, and an empty box means "Codex default".
  const [typed, setTyped] = useState(settings.model ?? '');
  useEffect(() => setTyped(settings.model ?? ''), [settings.model]);
  const commit = () => {
    const v = typed.trim() || null;
    if (v !== settings.model) pick('model', v);
  };
  const preset = CODEX_PRESETS.find(p => p.approval === settings.approval && p.sandbox === settings.sandbox);

  return (
    <>
      <div>
        <div style={labelStyle}>Model</div>
        <input
          type="text"
          list="codex-models"
          value={typed}
          placeholder={DEFAULT_MODEL_LABEL}
          onChange={e => setTyped(e.target.value)}
          onBlur={commit}
          onKeyDown={e => {
            // Enter also creates the chat (the dialog listens in capture);
            // commit first so what was typed is what gets created.
            if (e.key === 'Enter') commit();
          }}
          spellCheck={false}
          style={{ width: '100%' }}
        />
        <datalist id="codex-models">
          {recent.map(m => <option key={m} value={m} />)}
        </datalist>
        {origin('model')}
      </div>

      <div>
        <div style={labelStyle}>Reasoning effort</div>
        <Segmented items={CODEX_EFFORTS} value={settings.effort} onPick={(v: CodexEffort) => pick('effort', v)} />
        {origin('effort')}
      </div>

      <div>
        <div style={{ ...labelStyle, display: 'flex', alignItems: 'baseline', gap: 8 }}>
          <span>Approval &amp; sandbox</span>
          <span style={{ flex: 1 }} />
          {CODEX_PRESETS.map(p => (
            <span
              key={p.label}
              onClick={() => {
                pick('approval', p.approval);
                pick('sandbox', p.sandbox);
              }}
              title={p.hint}
              className="hover-bg"
              style={{
                fontWeight: preset?.label === p.label ? 700 : 400, cursor: 'default', borderRadius: 2,
                padding: '0 3px', color: preset?.label === p.label ? 'var(--fg)' : 'var(--dim)'
              }}
            >
              {p.label}
            </span>
          ))}
        </div>
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

export function Chips({ chat, chip, softChip }: { chat: CodexChat; chip: CSSProperties; softChip: CSSProperties }) {
  const model = chat.model ?? chat.modelSeen ?? null;
  const bypass = chat.approval === 'never' && chat.sandbox === 'danger-full-access';
  return (
    <>
      <span
        title={chat.model ? 'Model' : `Model: ${DEFAULT_MODEL_LABEL}${chat.modelSeen ? ` (running ${chat.modelSeen})` : ''}`}
        style={{ ...softChip, maxWidth: 120, ...(chat.model ? {} : { opacity: 0.8 }) }}
      >
        {model ?? 'default'}
      </span>
      <span title={`Reasoning effort: ${chat.effort}`} style={softChip}>{chat.effort}</span>
      {bypass ? (
        <span title="Approval: never · Sandbox: danger-full-access (--dangerously-bypass-approvals-and-sandbox)" style={chip}>bypass</span>
      ) : (
        <>
          <span title={`Approval: ${chat.approval} — ${CODEX_APPROVAL_HINTS[chat.approval]}`} style={softChip}>{chat.approval}</span>
          <span title={`Sandbox: ${chat.sandbox} — ${CODEX_SANDBOX_HINTS[chat.sandbox]}`} style={softChip}>{chat.sandbox}</span>
        </>
      )}
    </>
  );
}

export const summary = (chat: Chat) =>
  chat.provider === 'codex'
    ? `${chat.model ?? chat.modelSeen ?? DEFAULT_MODEL_LABEL} · ${chat.effort} · ${chat.approval} / ${chat.sandbox}`
    : '';

// -------------------------------------------------------------- accounts --

export function LimitBars({ account }: { account: Account }) {
  const lim = account.limits;
  if (lim.kind !== 'codex') return null;
  if (lim.windows.length === 0) {
    return (
      <div style={{ display: 'flex', alignItems: 'center', gap: 5, marginBottom: 3 }}>
        <span style={{ fontSize: 'var(--fs-1)', color: 'var(--dim)', width: 34, flex: 'none' }}>usage</span>
        <div className="xp-sunken" style={{ flex: 1, height: 6, background: 'var(--track)' }} />
        <span style={{ fontSize: 'var(--fs-1)', color: 'var(--dim)', width: 30, flex: 'none', textAlign: 'right' }}>—</span>
        <span style={{ width: 42, flex: 'none' }} />
      </div>
    );
  }
  return (
    <>
      {lim.windows.map(w => (
        <div key={w.id} style={{ display: 'flex', alignItems: 'center', gap: 5, marginBottom: 3 }} title={w.label}>
          <span style={{ fontSize: 'var(--fs-1)', color: 'var(--dim)', width: 34, flex: 'none', whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis' }}>{w.label}</span>
          <div className="xp-sunken" style={{ flex: 1, height: 6, background: 'var(--track)', overflow: 'hidden' }}>
            <div style={{ height: '100%', width: `${Math.round(w.used * 100)}%`, background: limitColor(w.used) }} />
          </div>
          <span style={{ fontSize: 'var(--fs-1)', color: 'var(--dim)', width: 30, flex: 'none', textAlign: 'right', fontVariantNumeric: 'tabular-nums' }}>
            {`${Math.round(w.used * 100)}%`}
          </span>
          <span style={{ fontSize: 'var(--fs-1)', color: 'var(--faint)', width: 42, flex: 'none', textAlign: 'right', whiteSpace: 'nowrap', overflow: 'hidden', fontVariantNumeric: 'tabular-nums' }}>
            {w.reset}
          </span>
        </div>
      ))}
    </>
  );
}

export const worstLimit = (account: Account) =>
  account.limits.kind === 'codex' ? Math.max(0, ...account.limits.windows.map(w => w.used)) : 0;

/** The longest window's reset — the weekly one where there is one. */
export const longResetAt = (account: Account) => {
  if (account.limits.kind !== 'codex') return null;
  const w = account.limits.windows.filter(w => w.id === 'primary' || w.id === 'secondary');
  return w.find(x => x.label === 'week')?.resetAt ?? w[w.length - 1]?.resetAt ?? null;
};
