/**
 * What deleting a chat does to the helpers its agent runs (DeleteChatDialog).
 * The chat's own boxes never reach further than the person said: a helper at
 * work is stopped only with a yes of its own, and a worktree holding unsaved work
 * goes only with one too. The core holds the same line (agents/teardown.rs
 * `delete_by_user`), against what changed since the dialog looked.
 */
import type { AtStakeDto, BackgroundHelperDto } from '../../ipc/agents';

/** What the person ticked. */
export interface HelperChoices {
  /** Delete the chat's helpers at all. */
  withHelpers: boolean;
  /** Drop the worktrees they made, as the chat's own box says. */
  dropWorktree: boolean;
  /** Stop and delete the ones still working too. */
  stopWorking: boolean;
  /** Drop worktrees with unsaved work in them too. */
  dropUnsaved: boolean;
}

/** One helper as the dialog lists it: what it is doing and what its worktree holds. */
export interface HelperLine {
  id: string;
  name: string;
  /** What a delete would cut off, in words; null when it is done. */
  working: string | null;
  /** Its worktree's unsaved work, in words; null with none or no worktree. */
  unsaved: string | null;
}

/** A state as a person reads it, for one that is at work. */
const WORKING: Record<string, string> = {
  working: 'working',
  waiting: 'asks for permission',
  question: 'asks a question',
  background: 'a job it started still runs'
};

/** Each helper with what it has at stake, from its row and the core's look (`stake`, null until in). */
export function helperLines(helpers: BackgroundHelperDto[], stake: Record<string, AtStakeDto> | null): HelperLine[] {
  return helpers.map(h => {
    const s = stake?.[h.id];
    return {
      id: h.id,
      name: h.name || h.id,
      working: s?.working ?? WORKING[h.state] ?? null,
      unsaved: s?.unsaved ? `${s.worktree ? `${s.worktree}: ` : ''}${s.unsaved}` : null
    };
  });
}

/** What goes, and how, for what was ticked. */
export interface HelperDelete {
  id: string;
  dropWorktree: boolean;
  stopWorking: boolean;
  dropUnsaved: boolean;
}

/** The helpers to delete and how; the rest are left to go by themselves once idle. */
export function helperDeletes(lines: HelperLine[], c: HelperChoices): HelperDelete[] {
  if (!c.withHelpers) return [];
  return lines
    .filter(l => !l.working || c.stopWorking)
    .map(l => ({
      id: l.id,
      dropWorktree: c.dropWorktree,
      stopWorking: !!l.working && c.stopWorking,
      dropUnsaved: c.dropWorktree && !!l.unsaved && c.dropUnsaved
    }));
}
