/**
 * Making a chat, in one place: the dialog and the no-dialog shortcut both come
 * through here, so they cannot drift on trust, naming, colour or placement.
 */
import type { Account, Chat, ClaudeSettings, CodexSettings } from '../../shared/types';
import { createWorktree } from '../../ipc/commands';
import { pickChatColor } from '../../shared/ui/chatColors';
import { newId, useChats, wornColors } from '../chats/chats.store';
import { findAccount, useAccounts } from '../accounts/accounts.store';
import { currentSlots, usePanes } from '../panes/panes.store';
import { claude, codex, ui } from '../providers';
import { useNewChat } from './newchat.store';

export type ChatSpec = {
  folder: string;
  account: Account;
  worktree: boolean;
  /** Attach Luna's MCP tools so the session can spawn and drive others. */
  tools?: boolean;
  /** Set when an agent spawns it: the row nests under this chat, and it is
   *  not seated in a pane — the parent is what the user is watching. */
  parentId?: string;
  name?: string;
} & ({ provider: 'claude'; settings: ClaudeSettings } | { provider: 'codex'; settings: CodexSettings });

/**
 * Adds the chat and seats it where it will be seen. Throws if the folder has
 * to be marked trusted and that write fails — neither CLI can show its own
 * trust prompt the way Luna runs it (Claude Code refuses under `--worktree`,
 * Codex would follow it with a sandbox setup question), so the bit has to
 * land before the session spawns. Also throws when a Codex worktree cannot
 * be made, which is the same kind of chat: one that would sit there
 * refusing to run.
 */
export async function createChat(spec: ChatSpec): Promise<string> {
  const p = ui(spec.provider);
  const accountPath = spec.account.path;
  if (accountPath && !(await p.folderTrusted(accountPath, spec.folder))) {
    await p.trustFolder(accountPath, spec.folder);
  }

  // Claude Code makes its own worktree from `--worktree`; Codex gets one made
  // for it here, before the row exists, so the path is known from the start.
  let worktreePath: string | null = null;
  if (spec.provider === 'codex' && spec.worktree) {
    worktreePath = await createWorktree(spec.folder);
    codex.rememberModel(spec.settings.model);
  }

  const { folders } = useChats.getState();
  const group = usePanes.getState().group;
  const n = folders.reduce((a, f) => a + f.chats.filter(c => c.group === group).length, 0) + 1;
  const id = newId('c');

  // A child lives in its parent's group, wherever the user is looking now.
  const parent = spec.parentId ? useChats.getState().findChat(spec.parentId) : null;
  const base = {
    id,
    name: spec.name?.trim() || `chat ${n}`,
    nameCustom: !!spec.name?.trim(),
    status: 'resting' as const,
    context: 0,
    account: spec.account.name,
    group: parent?.group ?? group,
    worktree: spec.worktree,
    worktreePath,
    color: pickChatColor(wornColors(folders, group)),
    tools: spec.tools ?? false,
    parentId: spec.parentId ?? null
  };
  const chat: Chat =
    spec.provider === 'claude'
      ? { ...base, provider: 'claude', ...spec.settings }
      : { ...base, provider: 'codex', ...spec.settings };

  useChats.getState().addChat(spec.folder, chat);
  if (!spec.parentId) {
    usePanes.getState().autoPlace(id);
    useNewChat.getState().remember(spec.folder, { provider: spec.provider, name: spec.account.name }, spec.worktree);
  }
  return id;
}

/** The chat on screen, if any — the one a new chat should take after. */
function workingOn() {
  const panes = usePanes.getState();
  const chats = useChats.getState();
  const inPane = currentSlots(panes)[panes.activePane];
  return chats.findChat(panes.peekChat ?? inPane ?? chats.active);
}

/**
 * The no-dialog path. Everything is already decided: the folder and account of
 * the chat on screen, and the rest from the settings files that would govern
 * it anyway. Falls back to the dialog when there is nothing to go on, or when
 * the trust write fails and the reason wants showing.
 */
export async function quickChat(): Promise<void> {
  const chats = useChats.getState();
  const accounts = useAccounts.getState().accounts;
  const ui = useNewChat.getState();
  const near = workingOn();

  const folder =
    (near ? chats.folderOf(near.id)?.path : null) ??
    ui.lastFolder ??
    chats.folders.find(f => f.chats.length > 0)?.path ??
    chats.folders[0]?.path ??
    '';
  const account =
    (near ? findAccount(accounts, near.provider, near.account) : null) ??
    (ui.lastAccount ? findAccount(accounts, ui.lastAccount.provider, ui.lastAccount.name) : null) ??
    accounts[0] ??
    null;

  if (!folder || !account) {
    ui.openDialog(folder || undefined);
    return;
  }

  const worktree = near?.worktree ?? ui.lastWorktree;
  try {
    if (account.provider === 'claude') {
      const { values } = await claude.settingsDefaults(account.path, folder);
      await createChat({ provider: 'claude', folder, account, settings: values, worktree });
    } else {
      const { values } = await codex.settingsDefaults(account.path, folder);
      await createChat({ provider: 'codex', folder, account, settings: values, worktree });
    }
  } catch {
    // The trust write or worktree failed; the dialog is where that has a
    // place to be said.
    ui.openDialog(folder);
  }
}
