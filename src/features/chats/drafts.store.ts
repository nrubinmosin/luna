import { create } from 'zustand';

/** What a running chat's composer holds up, as the core reports it
 *  (`SessionMetaDto.draft`, `held`). */
export interface DraftState {
  /** A person left something typed there and has not sent it. */
  draft: boolean;
  /** Messages Luna keeps back for it: sends from other sessions waiting to
   *  be typed in, notices waiting for the draft to go. */
  held: number;
}

/**
 * Drafts and held messages by chat id, filled by useSessionWatch. Not kept
 * with the chats: it is about the session running now, and a draft left
 * behind by a restart is the CLI's, not Luna's to remember.
 */
export const useDrafts = create<{
  by: Record<string, DraftState>;
  set: (chatId: string, state: DraftState | null) => void;
}>(set => ({
  by: {},
  set: (chatId, state) =>
    set(s => {
      const was = s.by[chatId];
      if (!state || (!state.draft && state.held === 0)) {
        if (!was) return s;
        const by = { ...s.by };
        delete by[chatId];
        return { by };
      }
      if (was && was.draft === state.draft && was.held === state.held) return s;
      return { by: { ...s.by, [chatId]: state } };
    })
}));

/** The tag a chat row and a pane title bar wear for it, with its tooltip;
 *  null when there is nothing to say. */
export function draftTag(d: DraftState | undefined): { text: string; title: string } | null {
  if (!d) return null;
  const msgs = `${d.held} message${d.held === 1 ? '' : 's'}`;
  if (d.draft && d.held > 0) {
    return {
      text: `draft · ${d.held}`,
      title: `An unsent draft is in the composer, and Luna is holding ${msgs} for this chat until it is sent or cleared.`
    };
  }
  if (d.draft) {
    return {
      text: 'draft',
      title: 'An unsent draft is in the composer. Messages from other sessions wait for it, unless Claude Code can put it aside (Ctrl+S) for them.'
    };
  }
  return { text: `${d.held} held`, title: `Luna is holding ${msgs} for this chat until you stop typing.` };
}
