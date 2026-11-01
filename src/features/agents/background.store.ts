/**
 * The helpers agents run in the background (background.rs): no rows of their
 * own, so the sidebar shows them only as a count on their parent's row. The
 * core sends the whole list on every change (`agent://background`); the
 * bridge keeps it here.
 */
import { create } from 'zustand';
import type { BackgroundHelperDto } from '../../ipc/commands';

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
export function rootOf(h: BackgroundHelperDto, all: BackgroundHelperDto[]): string | null {
  const byId = new Map(all.map(x => [x.id, x]));
  let parent = h.parent;
  // The core caps a line at a few generations; this only guards against a loop.
  for (let i = 0; i < 16 && parent; i++) {
    const up = byId.get(parent);
    if (!up) return parent;
    parent = up.parent;
  }
  return parent;
}
