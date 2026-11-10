/**
 * What a chat's helpers come to at a glance: the counts its pane's title bar
 * shows, the names that fit beside them, and the two tabs of their list.
 * Pure, so the bar, the list and their tests read the same rules.
 */
import type { BackgroundHelperDto } from '../../ipc/agents';
import { sinceLabel } from '../../shared/lib/format';

/** The list's two tabs. A helper that asks is at work: its turn waits on the answer. */
export type HelperTab = 'working' | 'idle';

/** A state as a person reads it. */
export const STATE_LABEL: Record<string, string> = {
  starting: 'starting',
  working: 'working',
  waiting: 'asks for permission',
  question: 'asks a question',
  idle: 'done',
  background: 'a job still runs',
  exited: 'gone'
};

/** It waits on an answer to go on (agents/ask.rs `state_of`). */
export const asks = (h: BackgroundHelperDto) => h.state === 'question' || h.state === 'waiting';

/** Which tab a helper is listed under; `null` for one that is gone, which no count takes in. */
export function tabOf(h: BackgroundHelperDto): HelperTab | null {
  switch (h.state) {
    case 'starting':
    case 'working':
    case 'background':
    case 'question':
    case 'waiting':
      return 'working';
    case 'idle':
      return 'idle';
    default:
      return null;
  }
}

export interface HelperCounts {
  working: number;
  idle: number;
  /** Of the working, those that ask. */
  asks: number;
}

export function countHelpers(helpers: BackgroundHelperDto[]): HelperCounts {
  const c = { working: 0, idle: 0, asks: 0 };
  for (const h of helpers) {
    const tab = tabOf(h);
    if (tab) c[tab]++;
    if (tab === 'working' && asks(h)) c.asks++;
  }
  return c;
}

/** Asking ones first, then the rest in the order they came. */
const asksFirst = (helpers: BackgroundHelperDto[]) => [...helpers.filter(asks), ...helpers.filter(h => !asks(h))];

/** The helpers a tab lists. Idle also lists the gone, which nothing counts but
 *  the list still offers to delete. */
export function helpersIn(helpers: BackgroundHelperDto[], tab: HelperTab): BackgroundHelperDto[] {
  if (tab === 'working') return asksFirst(helpers.filter(h => tabOf(h) === 'working'));
  return helpers.filter(h => tabOf(h) !== 'working');
}

/** The tab a list opens on: Working while anything works, else Idle. */
export const defaultTab = (helpers: BackgroundHelperDto[]): HelperTab =>
  helpers.some(h => tabOf(h) === 'working') ? 'working' : 'idle';

export const helperName = (h: BackgroundHelperDto) => h.name || h.id;

/** The names the bar has room for after its counts: asking ones first, then
 *  the working with how long their turn has run ("review-plan 4m"). */
export function summaryNames(
  helpers: BackgroundHelperDto[],
  now: number
): { id: string; asks: boolean; text: string }[] {
  return asksFirst(helpers.filter(h => tabOf(h) === 'working')).map(h => ({
    id: h.id,
    asks: asks(h),
    text: asks(h) || !h.turnStartedMs ? helperName(h) : `${helperName(h)} ${sinceLabel(h.turnStartedMs, now)}`
  }));
}

/** The counts in words, zero ones left out: "2 working · 1 idle · 1 asks". */
export function countWords(c: HelperCounts): string[] {
  return [c.working && `${c.working} working`, c.idle && `${c.idle} idle`, c.asks && `${c.asks} asks`].filter(
    (w): w is string => !!w
  );
}

/** The whole summary for a screen reader and the tooltip, names included. */
export function summaryLabel(helpers: BackgroundHelperDto[], now: number): string {
  const c = countHelpers(helpers);
  const gone = helpers.length - c.working - c.idle;
  const parts = [c.working && `${c.working} working`, c.asks && `${c.asks} of them asking`, c.idle && `${c.idle} idle`]
    .concat(gone && `${gone} gone`)
    .filter(Boolean);
  const asking = helpers.filter(asks).map(helperName);
  const working = summaryNames(helpers, now)
    .filter(n => !n.asks)
    .map(n => n.text);
  return [
    `Background helpers: ${parts.join(', ') || 'none'}.`,
    asking.length ? `Asking: ${asking.join(', ')}.` : '',
    working.length ? `Working: ${working.join(', ')}.` : '',
    'Hover, focus or click for the list.'
  ]
    .filter(Boolean)
    .join(' ');
}

/** What it runs on: "sonnet · medium · team · worktree". */
export function setupLine(h: BackgroundHelperDto): string {
  return [h.model ?? 'default model', h.effort, h.account, h.worktree ? 'worktree' : null].filter(Boolean).join(' · ');
}

/** "working 4m" on its turn, "done · up 12m" after it: since the turn or the spawn. */
export function stateLine(h: BackgroundHelperDto, now: number): string {
  const label = STATE_LABEL[h.state] ?? h.state;
  if (h.turnStartedMs) return `${label} ${sinceLabel(h.turnStartedMs, now)}`;
  return `${label} · up ${sinceLabel(h.startedMs, now)}`;
}

/** "active 2m ago", off the last thing Luna saw it do. */
export const activityLine = (h: BackgroundHelperDto, now: number): string | null =>
  h.lastActivityMs ? `active ${sinceLabel(h.lastActivityMs, now)} ago` : null;

/** What a helper asks, on one line. */
export function askLine(h: BackgroundHelperDto): string | null {
  const a = h.ask;
  if (!a) return null;
  if (a.kind === 'permission') return [a.tool, a.detail].filter(Boolean).join(': ') || null;
  return a.questions?.map(q => q.text).join(' · ') ?? null;
}
