/**
 * A background helper's conversation as the window shows it, read-only: read off
 * its transcript by the core (background/rows.rs `background_transcript`) from a
 * cursor on, and looked at again while the view is open — often while a turn
 * runs, now and then between turns, as a person may send it on its way.
 */
import { useEffect, useState } from 'react';
import { backgroundTranscript, type HelperMessageDto } from '../../ipc/agents';

/** How often the view looks while a turn runs, and between turns. */
export const TAIL_RUNNING_MS = 1000;
export const TAIL_IDLE_MS = 5000;

/** The most messages the view keeps: the newest. */
export const VIEW_KEEP = 400;

export interface HelperTranscript {
  messages: HelperMessageDto[];
  running: boolean;
  /** Older messages left out, by the core's first look or by VIEW_KEEP. */
  leftOut: number;
  /** Some of the conversation was never read: the core reads only so far
   *  back, and passes over a line too long to show. */
  passedOver: boolean;
  /** Nothing has come in yet. */
  loading: boolean;
  error: string | null;
}

/**
 * Tails helper `id`'s conversation for as long as the caller is mounted and
 * `open`. Each look reads only what came since the last (`cursor`); a new
 * transcript file starts the view over.
 */
export function useHelperTranscript(id: string, open = true): HelperTranscript {
  const [state, setState] = useState<HelperTranscript>({
    messages: [],
    running: false,
    leftOut: 0,
    passedOver: false,
    loading: true,
    error: null
  });
  useEffect(() => {
    if (!open) return;
    let live = true;
    let cursor: number | null = null;
    let file: string | null = null;
    let timer: number | undefined;
    const look = async () => {
      try {
        const t = await backgroundTranscript(id, cursor, file);
        if (!live) return;
        const fresh = t.file !== file;
        cursor = t.cursor;
        file = t.file;
        setState(s => {
          const all = fresh ? t.messages : [...s.messages, ...t.messages];
          const cut = Math.max(0, all.length - VIEW_KEEP);
          const leftOut = (fresh ? 0 : s.leftOut) + (t.leftOut ?? 0) + cut;
          const passedOver = (fresh ? false : s.passedOver) || !!t.passedOver;
          return {
            messages: cut ? all.slice(cut) : all,
            running: t.running,
            leftOut,
            passedOver,
            loading: false,
            error: null
          };
        });
        timer = window.setTimeout(() => void look(), t.running ? TAIL_RUNNING_MS : TAIL_IDLE_MS);
      } catch (e) {
        if (!live) return;
        // Gone (deleted, or opened as a chat): nothing more to show.
        setState(s => ({ ...s, running: false, loading: false, error: e instanceof Error ? e.message : String(e) }));
      }
    };
    void look();
    return () => {
      live = false;
      window.clearTimeout(timer);
    };
  }, [id, open]);
  return state;
}
