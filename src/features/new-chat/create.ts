/**
 * Making a chat, in one place: the dialog and the no-dialog shortcut both come
 * through here, so they cannot drift on trust, naming, colour or placement.
 */
import type { Effort, ModelLabel, PermMode } from '../../shared/types';
import { effortFromSetting, modelFromSetting, permFromSetting } from '../../shared/types';
import { claudeDefaults, folderTrusted, trustFolder, type DefaultSource } from '../../ipc/commands';
import { pickChatColor } from '../../shared/ui/chatColors';
import { newId, useChats, wornColors } from '../chats/chats.store';
import { useAccounts } from '../accounts/accounts.store';
import { currentSlots, usePanes } from '../panes/panes.store';
import { useNewChat } from './newchat.store';

/** The three things a settings file can have an opinion about. */
export interface ChatSettings {
  model: ModelLabel;
  effort: Effort;
  perm: PermMode;
}

/** Which file each resolved value came from; absent means Luna's own stock. */
export type SettingsFrom = Partial<Record<keyof ChatSettings, DefaultSource>>;

/** Where a chat opens when no settings file says otherwise. */
export const STOCK: ChatSettings = { model: 'Opus', effort: 'high', perm: 'Bypass' };

export const SOURCE_LABELS: Record<DefaultSource, string> = {
  account: "the account's settings.json",
  project: "the project's .claude/settings.json",
  'project-local': "the project's .claude/settings.local.json",
  managed: 'the machine-wide managed settings'
};

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
): Promise<{ values: ChatSettings; from: SettingsFrom }> {
  const dto = await claudeDefaults(accountPath, folder).catch(() => null);
  const values: ChatSettings = { ...STOCK };
  const from: SettingsFrom = {};

  const model = dto?.model ? modelFromSetting(dto.model.value) : null;
  if (model && dto?.model) {
    values.model = model;
    from.model = dto.model.source;
  }
  const effort = dto?.effort ? effortFromSetting(dto.effort.value) : null;
  if (effort && dto?.effort) {
    values.effort = effort;
    from.effort = dto.effort.source;
  }
  const perm = dto?.permissionMode ? permFromSetting(dto.permissionMode.value) : null;
  if (perm && dto?.permissionMode) {
    values.perm = perm;
    from.perm = dto.permissionMode.source;
  }
  return { values, from };
}

export interface ChatSpec extends ChatSettings {
  folder: string;
  account: string;
  worktree: boolean;
}

/**
 * Adds the chat and seats it where it will be seen. Throws only if the folder
 * has to be marked trusted and that write fails — the CLI cannot show its own
 * trust prompt under `--worktree`, so the bit has to land before the session
 * spawns, and a chat created without it would just sit there refusing to run.
 */
export async function createChat(spec: ChatSpec): Promise<string> {
  const accountPath = useAccounts.getState().accounts.find(a => a.name === spec.account)?.path ?? '';
  if (accountPath && !(await folderTrusted(accountPath, spec.folder))) {
    await trustFolder(accountPath, spec.folder);
  }

  const { folders } = useChats.getState();
  const group = usePanes.getState().group;
  const n = folders.reduce((a, f) => a + f.chats.filter(c => c.group === group).length, 0) + 1;
  const id = newId('c');

  useChats.getState().addChat(spec.folder, {
    id,
    name: `chat ${n}`,
    status: 'resting',
    model: spec.model,
    effort: spec.effort,
    perm: spec.perm,
    context: 0,
    account: spec.account,
    group,
    worktree: spec.worktree,
    color: pickChatColor(wornColors(folders, group))
  });
  usePanes.getState().autoPlace(id);
  useNewChat.getState().remember(spec.folder, spec.account, spec.worktree);
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
 * the chat on screen, and model/effort/permission mode from the settings files
 * that would govern it anyway. Falls back to the dialog when there is nothing
 * to go on, or when the trust write fails and the reason wants showing.
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
    (near && accounts.some(a => a.name === near.account) ? near.account : null) ??
    (ui.lastAccount && accounts.some(a => a.name === ui.lastAccount) ? ui.lastAccount : null) ??
    accounts[0]?.name ??
    '';

  if (!folder || !account) {
    ui.openDialog(folder || undefined);
    return;
  }

  const accountPath = accounts.find(a => a.name === account)?.path ?? '';
  const { values } = await settingsDefaults(accountPath, folder);
  try {
    await createChat({ folder, account, ...values, worktree: near?.worktree ?? ui.lastWorktree });
  } catch {
    // The trust write failed; the dialog is where that has a place to be said.
    ui.openDialog(folder);
  }
}
