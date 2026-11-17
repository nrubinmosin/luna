/**
 * The helpers agents run in the background (background/): no rows of their
 * own, so the sidebar shows them only as a count on their parent's row. The
 * core sends the whole list on every change (`agent://background`); the
 * bridge keeps it here.
 */
import { create } from 'zustand';
import type { BackgroundHelperDto } from '../../ipc/agents';

interface BackgroundState {
  helpers: BackgroundHelperDto[];
  setHelpers: (helpers: BackgroundHelperDto[]) => void;
}

export const useBackground = create<BackgroundState>()(set => ({
  helpers: [],
  setHelpers: helpers => set({ helpers })
}));

/**
 * What a helper hangs from in the window: its parent, or up through helpers
 * (which have no rows) to the first that is not one. `null` when the line ends
 * with no parent at all; a chat that is gone reads the same to the caller,
 * which knows the chats.
 */
function rootIn(h: BackgroundHelperDto, byId: ReadonlyMap<string, BackgroundHelperDto>): string | null {
  let parent = h.parent;
  // The core caps a line at a few generations; this only guards against a loop.
  for (let i = 0; i < 16 && parent; i++) {
    const up = byId.get(parent);
    if (!up) return parent;
    parent = up.parent;
  }
  return parent;
}

/** The helpers by what each hangs from (`rootIn`), `null` for those that hang
 *  from nothing, each list in the snapshot's order. */
export type HelperRoots = ReadonlyMap<string | null, BackgroundHelperDto[]>;

const roots = new WeakMap<BackgroundHelperDto[], HelperRoots>();

/** The root index of a snapshot of the helpers, built once per snapshot (the
 *  core sends a new list on every change): every chat row and pane asks it on
 *  each store change, and walking all the helpers for each was quadratic. */
export function helperRoots(all: BackgroundHelperDto[]): HelperRoots {
  let index = roots.get(all);
  if (index) return index;
  const byId = new Map(all.map(x => [x.id, x]));
  const built = new Map<string | null, BackgroundHelperDto[]>();
  for (const h of all) {
    const root = rootIn(h, byId);
    const list = built.get(root);
    if (list) list.push(h);
    else built.set(root, [h]);
  }
  roots.set(all, (index = built));
  return index;
}

const NONE: BackgroundHelperDto[] = [];

/** The helpers hanging from `parentId`, its helpers' helpers with them: the
 *  same array for as long as the snapshot is the same. */
export const helpersUnder = (all: BackgroundHelperDto[], parentId: string): BackgroundHelperDto[] =>
  helperRoots(all).get(parentId) ?? NONE;
