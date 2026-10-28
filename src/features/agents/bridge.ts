/**
 * The frontend's half of the agent layer (agents.rs). The core owns the
 * sessions and the lineage, the frontend owns the chat list — so a spawn
 * an agent asks for arrives here as `agent://spawn`, becomes a chat row and
 * a running session, and is answered with `agent_spawned`. A delete the
 * core has already carried out arrives as `agent://deleted` and only has
 * to drop the row.
 */
import { useEffect } from 'react';
import {
  accountUsage, agentSpawned, backgroundHelpers, chatNames, codexModels, deleteSession, ensureClaudeSession, ensureCodexSession,
  type AgentSpawnRequest, type CodexModelDto, type PromotedDto
} from '../../ipc/commands';
import { onAgentDeleted, onAgentPromote, onAgentSpawn, onBackgroundHelpers } from '../../ipc/events';
import { logWarn } from '../../shared/lib/log';
import {
  AUTO_REVIEW_SANDBOX, CODEX_APPROVALS, CODEX_SANDBOXES, EFFORTS, MODEL_CLI, MODELS, PERM_CLI, accountKey,
  codexApprovalFromSetting, codexEffortFromSetting, codexSandboxFromSetting, effortFromSetting, modelFromSetting, permFromSetting,
  type Account, type ClaudeSettings, type CodexSettings
} from '../../shared/types';
import { findAccount, useAccounts } from '../accounts/accounts.store';
import { allChats, useChats } from '../chats/chats.store';
import { createChat } from '../new-chat/create';
import type { From } from '../new-chat/draft';
import { usePanes } from '../panes/panes.store';
import { claude, codex } from '../providers';
import { useBackground } from './background.store';

/**
 * A setting the agent asked for and Luna cannot give: said back to it as the
 * spawn's error, rather than quietly replaced by a default it did not ask for.
 */
function refuse(why: string): never {
  throw new Error(why);
}

/** Given and not blank. */
const asked = (v: string | null | undefined): v is string => !!v?.trim();

/** The Claude settings asked for, over the account's defaults. A model may be
 *  an alias or a full id (`claude-sonnet-5-5`), read the way a settings
 *  file's `model` is. */
function claudeSettings(req: AgentSpawnRequest, values: ClaudeSettings): ClaudeSettings {
  if (asked(req.approval) || asked(req.sandbox)) refuse('approval and sandbox are Codex settings; Claude takes permissionMode');
  const model = asked(req.model)
    ? modelFromSetting(req.model) ??
      refuse(`unknown Claude model "${req.model}"; use ${MODELS.map(m => MODEL_CLI[m]).join(', ')} or a full id such as claude-opus-5`)
    : values.model;
  const effort = asked(req.effort)
    ? effortFromSetting(req.effort.trim()) ?? refuse(`unknown Claude effort "${req.effort}"; use ${EFFORTS.join(', ')}`)
    : values.effort;
  const perm = asked(req.permissionMode)
    ? permFromSetting(req.permissionMode.trim()) ??
      refuse(`unknown permission mode "${req.permissionMode}"; use ${Object.values(PERM_CLI).join(', ')}`)
    : values.perm;
  return { model, effort, perm };
}

/** The Codex settings asked for, over the account's defaults. The model and
 *  its effort are checked against the models Codex lists for the account,
 *  when it has listed any yet. */
async function codexSettings(
  req: AgentSpawnRequest,
  { values, from }: { values: CodexSettings; from: From<CodexSettings> },
  account: Account
): Promise<CodexSettings> {
  if (asked(req.permissionMode)) refuse('permissionMode is a Claude setting; Codex takes approval and sandbox');
  const approval = asked(req.approval)
    ? codexApprovalFromSetting(req.approval.trim()) ??
      refuse(`unknown approval "${req.approval}"; use ${CODEX_APPROVALS.join(', ')}`)
    : values.approval;
  const sandbox = asked(req.sandbox)
    ? codexSandboxFromSetting(req.sandbox.trim()) ??
      refuse(`unknown sandbox "${req.sandbox}"; use ${CODEX_SANDBOXES.join(', ')}`)
    : approval === 'auto-review' ? AUTO_REVIEW_SANDBOX : values.sandbox;
  if (approval === 'auto-review' && sandbox !== AUTO_REVIEW_SANDBOX) {
    refuse(`auto-review runs in ${AUTO_REVIEW_SANDBOX} only; leave sandbox out`);
  }
  const model = asked(req.model) ? req.model.trim() : values.model;

  const models = await codexModels(account.path).catch((): CodexModelDto[] => []);
  const listed = models.find(m => m.slug === model);
  if (asked(req.model) && models.length > 0 && !listed) {
    refuse(`Codex on ${account.name} does not list model "${model}"; it has ${models.map(m => m.slug).join(', ')}`);
  }
  // As in the dialog: a model asked for brings its own default level, unless
  // a settings file names one.
  const fromFile = !!from.effort && from.effort !== codex.SOURCE_LABELS.codex;
  const effort = asked(req.effort)
    ? codexEffortFromSetting(req.effort) ?? refuse(`"${req.effort}" is not a Codex effort level`)
    : asked(req.model) && !fromFile && listed?.defaultEffort
      ? listed.defaultEffort
      : values.effort;
  if (asked(req.effort) && listed && listed.efforts.length > 0 && !listed.efforts.includes(effort)) {
    refuse(`${listed.slug} takes effort ${listed.efforts.join(', ')}, not "${effort}"`);
  }
  return { model, effort, approval, sandbox };
}

async function spawn(req: AgentSpawnRequest): Promise<void> {
  const chats = useChats.getState();
  // A caller without a row of its own — the dev caller of agents.rs, or a
  // chat whose row was dropped while its session lived on — gets a top-level
  // chat in the folder it asked for.
  const parent = chats.findChat(req.parentId);
  const account = findAccount(useAccounts.getState().accounts, req.provider, req.account);
  if (!account) throw new Error(`no ${req.provider} account named ${req.account}`);
  const folder = req.folder || (parent ? chats.folderOf(parent.id)?.path : undefined);
  if (!folder) throw new Error('no folder to run in');
  const worktree = req.worktree ?? false;
  const tools = req.tools ?? false;
  const name = req.name?.trim() || undefined;

  // Everything is checked before the row exists, so a refusal leaves nothing
  // behind.
  const spec =
    req.provider === 'claude'
      ? { provider: 'claude' as const, settings: claudeSettings(req, (await claude.settingsDefaults(account.path, folder)).values) }
      : {
          provider: 'codex' as const,
          settings: await codexSettings(req, await codex.settingsDefaults(account.path, folder), account)
        };
  const inWorktree = req.inWorktree ?? undefined;
  const id = await createChat({ ...spec, folder, account, worktree, tools, parentId: parent?.id, name, inWorktree, byAgent: true });
  const chat = useChats.getState().findChat(id);
  const common = {
    chatId: id,
    folder: chat?.worktreePath || folder,
    accountPath: account.path,
    tools,
    parent: parent?.id ?? req.parentId,
    prompt: req.prompt
  };
  try {
    if (spec.provider === 'claude') {
      await ensureClaudeSession({ ...common, ...spec.settings, worktree });
    } else {
      await ensureCodexSession({ ...common, ...spec.settings, worktree: worktree && !inWorktree });
    }
  } catch (e) {
    // No session to go with the row: a row that sits there failing to start
    // is one nobody asked for. A worktree made for it goes too, unused; one
    // it was handed is its caller's.
    void deleteSession(id, folder, chat?.worktreePath ?? null, !inWorktree).catch(() => {});
    useChats.getState().deleteChat(id);
    throw e;
  }
  // A top-level row comes into view only now that its session runs (see
  // `byAgent`).
  if (!parent) usePanes.getState().autoPlace(id);
  const made = useChats.getState().findChat(id);
  await agentSpawned(req.requestId, id, made?.name ?? null, null);
}

const samePath = (a: string, b: string) =>
  a.replace(/\//g, '\\').replace(/\\+$/, '').toLowerCase() === b.replace(/\//g, '\\').replace(/\\+$/, '').toLowerCase();

/**
 * A background helper the user opened as a chat: a row under its parent,
 * keeping the helper's id, and its conversation resumed when a pane first
 * shows it — the core has already let go of it.
 */
async function promote(p: PromotedDto): Promise<void> {
  const account = useAccounts.getState().accounts.find(a => a.provider === p.provider && samePath(a.path, p.accountPath));
  if (!account) throw new Error(`no ${p.provider} account at ${p.accountPath}`);
  const s = p.settings as Record<string, string | null | undefined>;
  const word = (k: string) => (typeof s[k] === 'string' ? (s[k] as string) : '');
  const defaults = (await (p.provider === 'claude' ? claude : codex).settingsDefaults(account.path, p.folder)).values;
  const spec =
    p.provider === 'claude'
      ? {
          provider: 'claude' as const,
          settings: {
            model: modelFromSetting(word('model')) ?? (defaults as ClaudeSettings).model,
            effort: effortFromSetting(word('effort')) ?? (defaults as ClaudeSettings).effort,
            perm: permFromSetting(word('permissionMode')) ?? (defaults as ClaudeSettings).perm
          }
        }
      : {
          provider: 'codex' as const,
          settings: {
            model: word('model') || (defaults as CodexSettings).model,
            effort: codexEffortFromSetting(word('effort')) ?? (defaults as CodexSettings).effort,
            approval: codexApprovalFromSetting(word('approval')) ?? (defaults as CodexSettings).approval,
            sandbox: codexSandboxFromSetting(word('sandbox')) ?? (defaults as CodexSettings).sandbox
          }
        };
  const parent = p.parent ? useChats.getState().findChat(p.parent) : null;
  const id = await createChat({
    ...spec,
    id: p.id,
    folder: p.folder,
    account,
    worktree: !!p.worktreePath,
    tools: p.tools,
    parentId: parent?.id,
    name: p.name ?? undefined,
    inWorktree: p.worktreePath ?? undefined,
    byAgent: true
  });
  if (p.sessionId) useChats.getState().setSessionId(id, p.sessionId);
  if (parent) useChats.getState().setChildrenOpen(parent.id, true);
  else usePanes.getState().autoPlace(id);
}

/** Mounted once in App: listens for the core's agent requests. */
export function useAgentBridge() {
  useEffect(() => {
    let dead = false;
    const offs: Array<() => void> = [];
    const { setHelpers } = useBackground.getState();
    void backgroundHelpers().then(rows => !dead && setHelpers(rows)).catch(() => {});
    void onBackgroundHelpers(setHelpers).then(off => (dead ? off() : offs.push(off)));
    void onAgentPromote(p => {
      promote(p).catch(e => logWarn('agents', `opening helper ${p.id} as a chat failed: ${String(e)}`));
    }).then(off => (dead ? off() : offs.push(off)));
    void onAgentSpawn(req => {
      spawn(req).catch(e => {
        logWarn('agents', `spawn for ${req.parentId} failed: ${String(e)}`);
        // The agent reads this as the tool's error: the message, not "Error: …".
        void agentSpawned(req.requestId, null, null, e instanceof Error ? e.message : String(e));
      });
    }).then(off => (dead ? off() : offs.push(off)));
    void onAgentDeleted(({ id }) => {
      // The session, attachments and worktree are already gone (agents.rs);
      // only the row is left to drop.
      if (useChats.getState().findChat(id)) useChats.getState().deleteChat(id);
    }).then(off => (dead ? off() : offs.push(off)));
    return () => {
      dead = true;
      offs.forEach(off => off());
    };
  }, []);

  // The core knows sessions by id only; `luna_sessions` names them with the
  // rows' names and places them in their window groups, sent now and again
  // whenever one changes.
  useEffect(() => {
    let last = '';
    const push = () => {
      const chats = allChats(useChats.getState().folders);
      const names = Object.fromEntries(chats.map(c => [c.id, c.name]));
      const groups = Object.fromEntries(chats.map(c => [c.id, c.group]));
      const key = JSON.stringify([names, groups]);
      if (key === last) return;
      last = key;
      void chatNames(names, groups);
    };
    push();
    return useChats.subscribe(push);
  }, []);

  // `luna_list` gives agents each account's usage as the Accounts window
  // shows it, so a helper goes where there is room; sent whenever it moves.
  useEffect(() => {
    let last = '';
    const push = () => {
      const usage = Object.fromEntries(
        useAccounts.getState().accounts.map(a => [
          accountKey(a.provider, a.name),
          {
            signedIn: a.signedIn,
            plan: a.plan,
            limits: a.haveUsage
              ? a.limits.map(l => ({ label: l.label, usedPct: Math.round(l.used * 100), resetsAt: l.resetAt }))
              : [],
            takenAt: a.haveUsage && a.fetchedAt ? new Date(a.fetchedAt).toISOString() : null
          }
        ])
      );
      const key = JSON.stringify(usage);
      if (key === last) return;
      last = key;
      void accountUsage(usage);
    };
    push();
    return useAccounts.subscribe(push);
  }, []);
}
