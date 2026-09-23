/**
 * The frontend's half of the agent layer (agents.rs). The core owns the
 * sessions and the lineage, the frontend owns the chat list — so a spawn
 * an agent asks for arrives here as `agent://spawn`, becomes a chat row and
 * a running session, and is answered with `agent_spawned`. A delete the
 * core has already carried out arrives as `agent://deleted` and only has
 * to drop the row.
 */
import { useEffect } from 'react';
import { agentSpawned, ensureClaudeSession, ensureCodexSession, type AgentSpawnRequest } from '../../ipc/commands';
import { onAgentDeleted, onAgentSpawn } from '../../ipc/events';
import { logWarn } from '../../shared/lib/log';
import {
  MODEL_CLI, PERM_CLI,
  type CodexApproval, type CodexEffort, type CodexSandbox, type Effort, type ModelLabel, type PermMode
} from '../../shared/types';
import { findAccount, useAccounts } from '../accounts/accounts.store';
import { useChats } from '../chats/chats.store';
import { createChat } from '../new-chat/create';
import { claude, codex } from '../providers';

const EFFORTS = ['low', 'medium', 'high', 'xhigh', 'max'];

/** The CLI's word for a model back to the label the chat row stores. */
function modelLabel(cli: string | null | undefined, fallback: ModelLabel): ModelLabel {
  if (!cli) return fallback;
  const hit = (Object.entries(MODEL_CLI) as Array<[ModelLabel, string]>).find(([, v]) => v === cli.toLowerCase());
  return hit ? hit[0] : fallback;
}

function permLabel(cli: string | null | undefined, fallback: PermMode): PermMode {
  if (!cli) return fallback;
  const hit = (Object.entries(PERM_CLI) as Array<[PermMode, string]>).find(([, v]) => v === cli);
  return hit ? hit[0] : fallback;
}

async function spawn(req: AgentSpawnRequest): Promise<void> {
  const chats = useChats.getState();
  const parent = chats.findChat(req.parentId);
  if (!parent) throw new Error('the requesting chat no longer exists');
  const account = findAccount(useAccounts.getState().accounts, req.provider, req.account);
  if (!account) throw new Error(`no ${req.provider} account named ${req.account}`);
  const folder = req.folder || chats.folderOf(parent.id)?.path;
  if (!folder) throw new Error('no folder to run in');
  const worktree = req.worktree ?? false;
  const tools = req.tools ?? false;
  const name = req.name?.trim() || undefined;

  let id: string;
  if (req.provider === 'claude') {
    const { values } = await claude.settingsDefaults(account.path, folder);
    const settings = {
      model: modelLabel(req.model, values.model),
      effort: (req.effort && EFFORTS.includes(req.effort) ? req.effort : values.effort) as Effort,
      perm: permLabel(req.permissionMode, values.perm)
    };
    id = await createChat({ provider: 'claude', folder, account, settings, worktree, tools, parentId: parent.id, name });
    const chat = useChats.getState().findChat(id);
    await ensureClaudeSession({
      chatId: id,
      folder: chat?.worktreePath || folder,
      accountPath: account.path,
      model: settings.model,
      effort: settings.effort,
      perm: settings.perm,
      worktree,
      tools,
      parent: parent.id,
      prompt: req.prompt
    });
  } else {
    const { values } = await codex.settingsDefaults(account.path, folder);
    const settings = {
      model: req.model ?? values.model,
      effort: (req.effort && EFFORTS.includes(req.effort) ? req.effort : values.effort) as CodexEffort,
      approval: (req.approval === 'never' || req.approval === 'on-request' ? req.approval : values.approval) as CodexApproval,
      sandbox: (['read-only', 'workspace-write', 'danger-full-access'].includes(req.sandbox ?? '')
        ? req.sandbox
        : values.sandbox) as CodexSandbox
    };
    id = await createChat({ provider: 'codex', folder, account, settings, worktree, tools, parentId: parent.id, name });
    const chat = useChats.getState().findChat(id);
    await ensureCodexSession({
      chatId: id,
      folder: chat?.worktreePath || folder,
      accountPath: account.path,
      model: settings.model,
      effort: settings.effort,
      approval: settings.approval,
      sandbox: settings.sandbox,
      tools,
      parent: parent.id,
      prompt: req.prompt
    });
  }
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
        void agentSpawned(req.requestId, null, null, String(e));
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
}
