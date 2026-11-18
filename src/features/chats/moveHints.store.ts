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
  /** The chat is gone: its limit and dismissal go too. (Moves are the poll's:
   *  it keeps `opening` to what the core still has.) */
  forgetChat: (chatId: string) => void;
}

/** The poll's moves by new chat id, with each one unchanged since the last
 *  read kept as the same object; the same record where nothing changed, so
 *  the panes reading it render only on a change. */
export function openingOf(was: Record<string, MoveStatusDto>, moves: MoveStatusDto[]): Record<string, MoveStatusDto> {
  const next: Record<string, MoveStatusDto> = {};
  let same = Object.keys(was).length === moves.length;
  for (const m of moves) {
    const old = was[m.newId];
    if (old && JSON.stringify(old) === JSON.stringify(m)) next[m.newId] = old;
    else {
      next[m.newId] = m;
      same = false;
    }
  }
  return same && Object.keys(next).length === moves.length ? was : next;
}

export const useMoveHints = create<MoveHints>(set => ({
  limits: {},
  dismissed: {},
  opening: {},
  setMoves: moves =>
    set(s => {
      const opening = openingOf(s.opening, moves);
      return opening === s.opening ? s : { opening };
    }),
  setLimit: (chatId, limit) =>
    set(s => ((s.limits[chatId] ?? null) === limit ? s : { limits: { ...s.limits, [chatId]: limit } })),
  dismiss: chatId => set(s => ({ dismissed: { ...s.dismissed, [chatId]: true } })),
  forgetChat: chatId =>
    set(s => {
      if (!(chatId in s.limits) && !(chatId in s.dismissed)) return s;
      const { [chatId]: _l, ...limits } = s.limits;
      const { [chatId]: _d, ...dismissed } = s.dismissed;
      return { limits, dismissed };
    })
}));
