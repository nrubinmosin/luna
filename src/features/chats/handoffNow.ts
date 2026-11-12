/**
 * The pane's "Hand off now": the person asks a chat with Luna's tools for a
 * handoff, threshold or not (agents/handoff `handoff_now`). The chat writes
 * one and goes on from it in a fresh conversation; nothing is reset unless it
 * does. Whether the person's ask is open is the core's word, polled with the
 * chat's state (`Chat.handoffAsk`): the window keeps no clock of its own, so
 * a reload or the person's /clear shows what the core holds. What the window
 * knows of the chat says when the item is offered; the core's refusal is the
 * one that counts, and is shown in the pane.
 */
import { create } from 'zustand';
import { handoffNow } from '../../ipc/handoff';
import type { MoveStatusDto } from '../../ipc/moving';
import type { Chat } from '../../shared/types';
import { resumeWhenIdle } from './autoHandoff';
import { useChats } from './chats.store';

interface HandoffNowState {
  /** The core's refusal, or a failed call, for the pane to show. */
  refused: Record<string, string>;
  setRefused: (chatId: string, why: string | null) => void;
}

const without = <T>(r: Record<string, T>, key: string) => {
  const next = { ...r };
  delete next[key];
  return next;
};

export const useHandoffNow = create<HandoffNowState>()(set => ({
  refused: {},
  setRefused: (chatId, why) =>
    set(s => ({ refused: why == null ? without(s.refused, chatId) : { ...s.refused, [chatId]: why } }))
}));

/** Whether the person's ask of `chat` is open in the core: waiting to be
 *  typed, or typed and not lapsed. */
export function askStands(chat: Chat): boolean {
  return chat.handoffAsk?.kind === 'manual';
}

/** The phases a move has ended in: before them it is under way, and the chat
 *  is asked for no handoff (agents/handoff `now_refusal`). */
const MOVE_OVER: MoveStatusDto['phase'][] = ['done', 'failed', 'canceled'];

/** Why the window does not offer "Hand off now" for `chat`, with what would
 *  clear the way, or null where it does. `moves` are the moves the window
 *  knows of. */
export function handoffNowRefusal(chat: Chat, moves: MoveStatusDto[]): string | null {
  if (!chat.tools)
    return "This chat runs without Luna's tools, which are what it hands off with: start a chat with the Luna tools box ticked.";
  if (chat.provider === 'claude' && chat.perm === 'Plan only')
    return 'In plan mode the chat writes no file, its handoff included: leave plan mode first.';
  if (chat.provider === 'codex' && (chat.sandbox === 'read-only' || chat.sandbox === 'read-only-net'))
    return "Codex's read-only sandbox writes no file, its handoff included: start the chat with workspace-write.";
  if (chat.handoff?.state === 'underWay') return `Handoff ${chat.handoff.n} of this chat is under way already.`;
  if (moves.some(m => m.chatId === chat.id && !MOVE_OVER.includes(m.phase)))
    return 'A move of this chat to another account is under way, and no handoff is asked while it is: let it finish or cancel it first.';
  if (askStands(chat)) {
    const lapses = chat.handoffAsk?.lapsesAt;
    return lapses
      ? `Asked already: the ask lapses at ${new Date(lapses).toLocaleTimeString()}, unless the chat hands off.`
      : 'Asked already: Luna types the request once the chat is at rest.';
  }
  return null;
}

/** Asks the core for the handoff; where its CLI has to start again on the same
 *  conversation for the tool to reach it, that is done once the chat is idle,
 *  as long as it keeps Luna's tools. The ask shows as the core's until its
 *  next poll says otherwise. */
export async function askHandoffNow(chatId: string): Promise<void> {
  const store = useHandoffNow.getState();
  store.setRefused(chatId, null);
  try {
    const { resume } = await handoffNow(chatId);
    useChats.getState().setHandoffAsk(chatId, { kind: 'manual', at: Date.now() });
    if (resume) resumeWhenIdle(chatId, c => !!c.tools);
  } catch (e) {
    useHandoffNow.getState().setRefused(chatId, String(e));
  }
}
