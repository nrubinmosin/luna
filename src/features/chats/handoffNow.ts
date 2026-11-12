/**
 * The pane's "Hand off now": the person asks a chat with Luna's tools for a
 * handoff, threshold or not (agents/handoff `handoff_now`). The chat writes
 * one and goes on from it in a fresh conversation; nothing is reset unless it
 * does. What the window knows of the chat says when the item is offered; the
 * core's refusal is the one that counts, and is shown in the pane.
 */
import { create } from 'zustand';
import { handoffNow } from '../../ipc/handoff';
import type { MoveStatusDto } from '../../ipc/moving';
import type { Chat } from '../../shared/types';
import { resumeWhenIdle } from './autoHandoff';

/** How long the person's ask holds in the core once its request is typed
 *  (agents/handoff `MANUAL_ASK_MS`): the pane says "asked" as long. */
export const ASK_HOLDS_MS = 10 * 60_000;

interface HandoffNowState {
  /** When the person last asked each chat, while the window counts it open. */
  asked: Record<string, number>;
  /** The core's refusal, or a failed call, for the pane to show. */
  refused: Record<string, string>;
  setAsked: (chatId: string, at: number | null) => void;
  setRefused: (chatId: string, why: string | null) => void;
}

const without = <T>(r: Record<string, T>, key: string) => {
  const next = { ...r };
  delete next[key];
  return next;
};

export const useHandoffNow = create<HandoffNowState>()(set => ({
  asked: {},
  refused: {},
  setAsked: (chatId, at) => set(s => ({ asked: at == null ? without(s.asked, chatId) : { ...s.asked, [chatId]: at } })),
  setRefused: (chatId, why) =>
    set(s => ({ refused: why == null ? without(s.refused, chatId) : { ...s.refused, [chatId]: why } }))
}));

/** Whether the person's ask of `chat`, made at `at`, still stands as the
 *  window sees it: not lapsed, and no handoff scheduled or ended since. */
export function askStands(chat: Chat, at: number | undefined, now = Date.now()): boolean {
  if (at == null || now - at > ASK_HOLDS_MS) return false;
  return !chat.handoff || chat.handoff.at < at;
}

/** Why the window does not offer "Hand off now" for `chat`, with what would
 *  clear the way, or null where it does. `moves` are the moves the window
 *  knows of; `askedAt`, when the person last asked it. */
export function handoffNowRefusal(
  chat: Chat,
  moves: MoveStatusDto[],
  askedAt?: number,
  now = Date.now()
): string | null {
  if (!chat.tools)
    return "This chat runs without Luna's tools, which are what it hands off with: start a chat with the Luna tools box ticked.";
  if (chat.provider === 'claude' && chat.perm === 'Plan only')
    return 'In plan mode the chat writes no file, its handoff included: leave plan mode first.';
  if (chat.provider === 'codex' && (chat.sandbox === 'read-only' || chat.sandbox === 'read-only-net'))
    return "Codex's read-only sandbox writes no file, its handoff included: start the chat with workspace-write.";
  if (chat.handoff?.state === 'underWay') return `Handoff ${chat.handoff.n} of this chat is under way already.`;
  if (moves.some(m => m.chatId === chat.id && (m.phase === 'asking' || m.phase === 'asked')))
    return "The chat is being asked for its move's document: let that move go on or cancel it first.";
  if (askStands(chat, askedAt, now))
    return `Asked already: the ask lapses ${ASK_HOLDS_MS / 60_000} minutes after Luna types it, unless the chat hands off.`;
  return null;
}

/** Asks the core for the handoff; where its CLI has to start again on the same
 *  conversation for the tool to reach it, that is done once the chat is idle,
 *  as long as it keeps Luna's tools. */
export async function askHandoffNow(chatId: string): Promise<void> {
  const store = useHandoffNow.getState();
  store.setRefused(chatId, null);
  try {
    const { resume } = await handoffNow(chatId);
    useHandoffNow.getState().setAsked(chatId, Date.now());
    if (resume) resumeWhenIdle(chatId, c => !!c.tools);
  } catch (e) {
    useHandoffNow.getState().setRefused(chatId, String(e));
  }
}
