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
