/**
 * A chat's auto-handoff, set from its What Luna applied panel while it runs:
 * kept on the chat for its next starts, told to the core at once (the
 * threshold, and the tool refused once off), and where its CLI runs without
 * the tool, that CLI started again on the same conversation as soon as the
 * chat is idle: a CLI lists its tools once, as it starts (agents/handoff).
 */
import { setAutoHandoff } from '../../ipc/handoff';
import { handoffOf, type Chat } from '../../shared/types';
import { accountOfChat, useAccounts } from '../accounts/accounts.store';
import { useRestarts } from '../panes/restarts.store';
import { resumeSession } from '../panes/terminals';
import { useChats } from './chats.store';
import { useDrafts } from './drafts.store';

/** How often a chat waiting for its resume is looked at. */
export const RESUME_LOOK_MS = 2000;

export async function applyAutoHandoff(chatId: string, on: boolean, at: number): Promise<void> {
  useChats.getState().setAutoHandoff(chatId, on, at);
  const chat = useChats.getState().findChat(chatId);
  if (!chat) return;
  const { resume } = await setAutoHandoff(chatId, handoffOf(chat)).catch(() => ({ resume: false }));
  if (resume) resumeWhenIdle(chatId);
  else useRestarts.getState().setPending(chatId, false);
}

/**
 * Starts the chat's CLI again once it is idle with nothing typed in it, unless
 * what asked for it no longer holds (`wanted`: auto-handoff still on, by
 * default; Hand off now asks only that the chat keeps Luna's tools) or the
 * chat is deleted meanwhile. Resting is what its row says; a draft or text
 * Luna holds for it waits, as a person's keys would.
 */
export function resumeWhenIdle(chatId: string, wanted: (chat: Chat) => boolean = c => !!c.autoHandoff): void {
  const restarts = useRestarts.getState();
  if (restarts.pending[chatId]) return;
  restarts.setPending(chatId, true);
  const look = () => {
    if (!useRestarts.getState().pending[chatId]) return;
    const chat = useChats.getState().findChat(chatId);
    const folder = useChats.getState().folderOf(chatId);
    const accountPath = chat ? accountOfChat(useAccounts.getState().accounts, chat)?.path : undefined;
    if (!chat || !folder || !accountPath || !wanted(chat)) {
      useRestarts.getState().setPending(chatId, false);
      return;
    }
    const drafts = useDrafts.getState().by[chatId];
    if (chat.status !== 'resting' || drafts?.draft || (drafts?.held ?? 0) > 0) {
      setTimeout(look, RESUME_LOOK_MS);
      return;
    }
    void resumeSession(chat, folder.path, accountPath).finally(() => useRestarts.getState().setPending(chatId, false));
  };
  setTimeout(look, RESUME_LOOK_MS);
}
