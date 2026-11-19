/**
 * What deleting a chat does to the helpers its agent runs (DeleteChatDialog).
 * The chat's own boxes never reach further than the person said: a helper at
 * work is stopped only with a yes of its own. The core holds the same line
 * (agents/teardown.rs `delete_by_user`), against what changed since the dialog
 * looked. A helper
 * passed to another chat goes there, the helpers under it with it, before the
 * chat is deleted, and none of them is deleted.
 */
import type { AtStakeDto, BackgroundHelperDto, MoveTargetsDto } from '../../ipc/agents';
import { suggestedChats } from '../agents/ownership';

/** What the person ticked. */
export interface HelperChoices {
  /** Delete the chat's helpers at all. */
  withHelpers: boolean;
  /** Stop and delete the ones still working too. */
  stopWorking: boolean;
}

/** One helper as the dialog lists it: what it is doing. */
export interface HelperLine {
  id: string;
  name: string;
  /** Whom it answers to: the chat, or a helper of it. */
  parent: string | null;
  /** What a delete would cut off, in words; null when it is done. */
  working: string | null;
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
      parent: h.parent,
      working: s?.working ?? WORKING[h.state] ?? null
    };
  });
}

/** What goes, and how, for what was ticked. */
export interface HelperDelete {
  id: string;
  stopWorking: boolean;
}

/** Each helper passed to a chat (`passing`, helper id to chat id), and every
 *  helper under one, which goes with it: none of them is deleted. */
export function movingWith(lines: HelperLine[], passing: Record<string, string>): Set<string> {
  const byId = new Map(lines.map(l => [l.id, l]));
  const moves = (l: HelperLine): boolean => {
    // The core caps a line at a few generations; this only guards against a loop.
    for (let at: HelperLine | undefined = l, i = 0; at && i < 16; at = byId.get(at.parent ?? ''), i++)
      if (passing[at.id]) return true;
    return false;
  };
  return new Set(lines.filter(moves).map(l => l.id));
}

/** The moves to make before the chat goes, in the list's order. */
export function helperMoves(
  lines: HelperLine[],
  passing: Record<string, string>
): { id: string; name: string; to: string }[] {
  return lines.flatMap(l => {
    const to = passing[l.id];
    return to ? [{ id: l.id, name: l.name, to }] : [];
  });
}

/** The chat a helper's picker starts on: the latest that was refused a write
 *  into it, else one waiting on it, if it may move there. Taken once, when its
 *  targets come in (DeleteChatDialog), so it never changes under the person's
 *  eyes; never moved unless the person confirms the dialog. */
export function preselectPass(h: BackgroundHelperDto | undefined, targets: MoveTargetsDto | undefined): string | null {
  if (!h || !targets || targets.blocked) return null;
  const ok = new Set(targets.targets.filter(t => t.ok).map(t => t.id));
  const asked = [...suggestedChats(h), ...(h.watchedBy ?? [])];
  return asked.find(c => ok.has(c)) ?? null;
}

/** The helpers to delete and how; the rest are left to go by themselves once
 *  idle, but for those `moving` to another chat. */
export function helperDeletes(
  lines: HelperLine[],
  c: HelperChoices,
  moving: ReadonlySet<string> = new Set(),
  shown?: ReadonlySet<string>
): HelperDelete[] {
  if (!c.withHelpers) return [];
  return (
    lines
      .filter(l => !moving.has(l.id))
      // Only what the dialog listed: a helper spawned under the chat since was never shown.
      .filter(l => !shown || shown.has(l.id))
      .filter(l => !l.working || c.stopWorking)
      .map(l => ({ id: l.id, stopWorking: !!l.working && c.stopWorking }))
  );
}
