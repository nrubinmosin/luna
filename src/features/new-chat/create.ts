/**
 * Making a chat, in one place: the dialog and the no-dialog shortcut both come
 * through here, so they cannot drift on trust, naming, color or placement.
 */
import { codexHooksStart } from '../../ipc/trust';
import type { Account, Chat, ClaudeSettings, CodexSettings } from '../../shared/types';
import { pickChatColor } from '../../shared/ui/chatColors';
import { newId, useChats, wornColors } from '../chats/chats.store';
import { findAccount, useAccounts } from '../accounts/accounts.store';
import { currentSlots, usePanes } from '../panes/panes.store';
import { claude, codex, ui, ui as providerUi } from '../providers';
import { startFor, useNewChat } from './newchat.store';

export type ChatSpec = {
  folder: string;
  account: Account;
  worktree: boolean;
  /** Attach Luna's MCP tools so the session can spawn and drive others. */
  tools?: boolean;
  /** The guard against endless waits (ticked in the dialog). */
  guard?: boolean;
  /** Mark the folder trusted in the account, as the CLI's own trust prompt
   *  would: ticked in the dialog, never for a helper opened as a chat. */
  trust?: boolean;
  /** Codex: run the project's hooks without their trust prompt. */
  hooksUnasked?: boolean;
  name?: string;
  /** An existing worktree of `folder` to run in — a helper's caller's. The
   *  chat is filed under the project and does not own the worktree. */
  inWorktree?: string;
  /** A helper opened as a chat: never remembered as the user's last choice,
   *  and seated by the caller once its conversation is known — seated now,
   *  its pane would start a new session instead of going on with that one. */
  fromHelper?: boolean;
  /** The id to take: a background helper's, opened as a chat, keeps its own,
   *  the one its transcript and Luna's log know it by. */
  id?: string;
  /** Claude Code: what the helper was spawned with, kept for every start. */
  readOnly?: boolean;
  agent?: string | null;
} & ({ provider: 'claude'; settings: ClaudeSettings } | { provider: 'codex'; settings: CodexSettings });

/**
 * Adds the chat and seats it where it will be seen. Marks the folder trusted
 * in the account first when the person ticked that (`trust`) and it is not
 * yet, and throws if that write fails: the mark has to land before the
 * session spawns. Unticked, nothing is written and the CLI asks itself. A
 * helper opened as a chat never writes it: a helper is trusted for its own
 * runs alone (the core's `codex_trust`).
 */
export async function createChat(spec: ChatSpec): Promise<string> {
  const p = ui(spec.provider);
  const accountPath = spec.account.path;
  if (spec.trust && !spec.fromHelper && accountPath && !(await p.folderTrusted(accountPath, spec.folder))) {
    await p.trustFolder(accountPath, spec.folder);
  }

  // Both CLIs make their own worktree from `--worktree`; its path is learned
  // from the session once it runs (useSessionWatch).
  const worktreePath: string | null = spec.inWorktree ?? null;

  const { folders } = useChats.getState();
  const group = usePanes.getState().group;
  const n = folders.reduce((a, f) => a + f.chats.filter(c => c.group === group).length, 0) + 1;
  const id = spec.id ?? newId('c');

  const base = {
    id,
    name: spec.name?.trim() || `chat ${n}`,
    nameSource: spec.name?.trim() ? ('user' as const) : undefined,
    status: 'resting' as const,
    context: 0,
    account: spec.account.name,
    group,
    worktree: spec.worktree,
    worktreePath,
    color: pickChatColor(wornColors(folders, group)),
    tools: spec.tools ?? false,
    guard: spec.guard ?? false
  };
  const chat: Chat =
    spec.provider === 'claude'
      ? { ...base, provider: 'claude', ...spec.settings, readOnly: spec.readOnly, agent: spec.agent }
      : { ...base, provider: 'codex', ...spec.settings, hooksUnasked: spec.hooksUnasked ?? false };

  useChats.getState().addChat(spec.folder, chat, !spec.fromHelper);
  if (!spec.fromHelper) {
    usePanes.getState().autoPlace(id);
    useNewChat.getState().remember(spec.folder, { provider: spec.provider, name: spec.account.name });
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
 * it anyway. Falls back to the dialog when there is nothing to go on, when the
 * folder is not trusted in the account (whether to trust it is the person's to
 * say, and the dialog is where it is asked), or when creating fails and the
 * reason wants showing.
 */
export async function quickChat(): Promise<void> {
  const chats = useChats.getState();
  const accounts = useAccounts.getState().accounts;
  const ui = useNewChat.getState();
  const near = workingOn();

  const folder =
    (near ? chats.folderOf(near.id)?.path : null) ??
    ui.lastFolder ??
    chats.folders.find(f => f.picked && f.chats.length > 0)?.path ??
    chats.folders.find(f => f.picked)?.path ??
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
  const trusted = await providerUi(account.provider)
    .folderTrusted(account.path, folder)
    .catch(() => false);
  if (!trusted) {
    ui.openDialog(folder);
    return;
  }

  // The folder's own start, else the defaults from Settings — not the chat on
  // screen, which may have been a one-off.
  const { worktree, tools } = startFor(chats.folders.find(f => f.path === folder));
  try {
    if (account.provider === 'claude') {
      const { values } = await claude.settingsDefaults(account.path, folder);
      await createChat({ provider: 'claude', folder, account, settings: values, worktree, tools });
    } else {
      const { values } = await codex.settingsDefaults(account.path, folder);
      const hooks = await codexHooksStart(folder);
      await createChat({
        provider: 'codex',
        folder,
        account,
        settings: values,
        worktree,
        tools,
        hooksUnasked: hooks.kept ?? hooks.default
      });
    }
  } catch {
    // The dialog is where the reason has a place to be said.
    ui.openDialog(folder);
  }
}
