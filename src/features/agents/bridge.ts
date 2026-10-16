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
  agentSpawned, chatNames, codexModels, deleteSession, ensureClaudeSession, ensureCodexSession,
  type AgentSpawnRequest, type CodexModelDto
} from '../../ipc/commands';
import { onAgentDeleted, onAgentSpawn } from '../../ipc/events';
import { logWarn } from '../../shared/lib/log';
import {
  CODEX_APPROVALS, CODEX_SANDBOXES, EFFORTS, MODEL_CLI, MODELS, PERM_CLI,
  codexApprovalFromSetting, codexEffortFromSetting, codexSandboxFromSetting, effortFromSetting, modelFromSetting, permFromSetting,
  type Account, type ClaudeSettings, type CodexSettings
} from '../../shared/types';
import { findAccount, useAccounts } from '../accounts/accounts.store';
import { allChats, useChats } from '../chats/chats.store';
import { createChat } from '../new-chat/create';
import { usePanes } from '../panes/panes.store';
import { claude, codex } from '../providers';

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
async function codexSettings(req: AgentSpawnRequest, values: CodexSettings, account: Account): Promise<CodexSettings> {
  if (asked(req.permissionMode)) refuse('permissionMode is a Claude setting; Codex takes approval and sandbox');
  const effort = asked(req.effort)
    ? codexEffortFromSetting(req.effort) ?? refuse(`"${req.effort}" is not a Codex effort level`)
    : values.effort;
  const approval = asked(req.approval)
    ? codexApprovalFromSetting(req.approval.trim()) ??
      refuse(`unknown approval "${req.approval}"; use ${CODEX_APPROVALS.join(', ')}`)
    : values.approval;
  const sandbox = asked(req.sandbox)
    ? codexSandboxFromSetting(req.sandbox.trim()) ??
      refuse(`unknown sandbox "${req.sandbox}"; use ${CODEX_SANDBOXES.join(', ')}`)
    : values.sandbox;
  const model = asked(req.model) ? req.model.trim() : values.model;

  const models = await codexModels(account.path).catch((): CodexModelDto[] => []);
  const listed = models.find(m => m.slug === model);
  if (asked(req.model) && models.length > 0 && !listed) {
    refuse(`Codex on ${account.name} does not list model "${model}"; it has ${models.map(m => m.slug).join(', ')}`);
  }
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
          settings: await codexSettings(req, (await codex.settingsDefaults(account.path, folder)).values, account)
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
      await ensureCodexSession({ ...common, ...spec.settings });
    }
  } catch (e) {
    // No session to go with the row: a row that sits there failing to start
    // is one nobody asked for. A worktree made for it goes too, unused; one
    // it was handed is its caller's.
    void deleteSession(id, folder, account.path, chat?.worktreePath ?? null, !inWorktree).catch(() => {});
    useChats.getState().deleteChat(id);
    throw e;
  }
  // A top-level row comes into view only now that its session runs (see
  // `byAgent`).
  if (!parent) usePanes.getState().autoPlace(id);
  const made = useChats.getState().findChat(id);
  await agentSpawned(req.requestId, id, made?.name ?? null, null);
}

/** Mounted once in App: listens for the core's agent requests. */
export function useAgentBridge() {
  useEffect(() => {
    let dead = false;
    const offs: Array<() => void> = [];
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
}
