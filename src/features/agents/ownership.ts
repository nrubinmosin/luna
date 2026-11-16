/**
 * Whose a helper is, as its row says it (background/rows.rs `Row`): who
 * manages it and which chat its line hangs from, which other chats wait on
 * it, where the person last moved it from, and the chats refused a write into
 * it that it could move to. Pure, so the list, the read-only view and the
 * Delete dialog read the same words.
 */
import type { BackgroundHelperDto } from '../../ipc/agents';
import { sinceLabel } from '../../shared/lib/format';

/** A session's name as the window shows it: a chat's row name, a helper's name, else its id. */
export type NameOf = (id: string) => string;

/** Names the window knows: the chats' rows and the helpers'. */
export function namer(chats: { id: string; name: string }[], helpers: BackgroundHelperDto[]): NameOf {
  const names = new Map<string, string>();
  for (const h of helpers) names.set(h.id, h.name || h.id);
  for (const c of chats) if (c.name) names.set(c.id, c.name);
  return id => names.get(id) ?? id;
}

/** "managed by <parent> · chat <top>", the parent and the top one chat: "managed by chat <top>". */
export function managedLine(h: BackgroundHelperDto, nameOf: NameOf): string | null {
  if (!h.parent) return null;
  if (!h.top) return `managed by ${nameOf(h.parent)}`;
  if (h.top === h.parent) return `managed by chat ${nameOf(h.top)}`;
  return `managed by ${nameOf(h.parent)} · chat ${nameOf(h.top)}`;
}

/** "moved from <chat>, 5m ago": the person's last move of it. */
export function movedLine(h: BackgroundHelperDto, nameOf: NameOf, now: number): string | null {
  const m = h.moved;
  if (!m) return null;
  return `moved from ${m.from ? nameOf(m.from) : 'no chat'}, ${sinceLabel(m.atMs, now)} ago`;
}

/** Its marks on one line: who watches it and where it came from, then who
 *  manages it. A helper's helper says who manages it first; a chat's own
 *  helper last, as the chat it is listed under says it already, and a narrow
 *  list cuts the line's end. */
export function ownershipLine(h: BackgroundHelperDto, nameOf: NameOf, now: number): string {
  const watched = h.watchedBy?.length ? `watched by ${h.watchedBy.map(nameOf).join(', ')}` : null;
  const managed = managedLine(h, nameOf);
  const marks = [watched, movedLine(h, nameOf, now)];
  return (h.top && h.parent !== h.top ? [managed, ...marks] : [...marks, managed]).filter(Boolean).join(' · ');
}

/** The chats it could move to on a click, the latest refused first, each once. */
export function suggestedChats(h: BackgroundHelperDto): string[] {
  return [...new Set((h.suggested ?? []).map(s => s.chat).reverse())];
}
