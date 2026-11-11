/**
 * What a pane offers a chat whose account runs low, or whose last turn a
 * usage limit stopped: a move to another account (features/chats/moving.ts).
 * The limit comes with each session look (`session_meta`); a dismissal holds
 * for this run of the window.
 */
import { create } from 'zustand';
import type { MoveStatusDto } from '../../ipc/moving';

interface MoveHints {
  /** The limit each chat's last turn stopped on, as its CLI words it. */
  limits: Record<string, string | null>;
  dismissed: Record<string, true>;
  /** Each move under way or ended lately, by its new chat's id, as the poll
   *  (chats/moving.ts `useMoves`) last read them. */
  opening: Record<string, MoveStatusDto>;
  setMoves: (moves: MoveStatusDto[]) => void;
  setLimit: (chatId: string, limit: string | null) => void;
  dismiss: (chatId: string) => void;
}

export const useMoveHints = create<MoveHints>(set => ({
  limits: {},
  dismissed: {},
  opening: {},
  setMoves: moves => set({ opening: Object.fromEntries(moves.map(m => [m.newId, m])) }),
  setLimit: (chatId, limit) =>
    set(s => ((s.limits[chatId] ?? null) === limit ? s : { limits: { ...s.limits, [chatId]: limit } })),
  dismiss: chatId => set(s => ({ dismissed: { ...s.dismissed, [chatId]: true } }))
}));
