import { create } from 'zustand';
import {
  applyRecommendation, dismissRecommendation, recommendations, type CheckDto
} from '../../ipc/commands';

interface State {
  /** Whether the Recommendations window is up. */
  open: boolean;
  setOpen: (v: boolean) => void;
  checks: CheckDto[] | null;
  /** What the last fix came back with, by the check's key. */
  failed: Record<string, string>;
  refresh: () => Promise<void>;
  apply: (c: CheckDto, confirmed: boolean) => Promise<void>;
  dismiss: (c: CheckDto, dismissed: boolean) => Promise<void>;
}

export const checkKey = (c: CheckDto) => `${c.id}@${c.account ?? ''}`;

/** What the chip counts: something wrong that was not left so on purpose. */
export const openChecks = (checks: CheckDto[] | null) =>
  (checks ?? []).filter(c => c.status !== 'ok' && !c.dismissed);

export const useRecommendations = create<State>((set, get) => ({
  open: false,
  setOpen: v => set({ open: v }),
  checks: null,
  failed: {},
  refresh: async () => {
    try {
      set({ checks: await recommendations() });
    } catch {
      // The core being away is said elsewhere; the list stays as it was.
    }
  },
  apply: async (c, confirmed) => {
    const key = checkKey(c);
    const { [key]: _gone, ...failed } = get().failed;
    set({ failed });
    try {
      set({ checks: await applyRecommendation(c.id, c.account, confirmed) });
    } catch (e) {
      set({ failed: { ...get().failed, [key]: String(e) } });
      await get().refresh();
    }
  },
  dismiss: async (c, dismissed) => {
    try {
      set({ checks: await dismissRecommendation(c.id, c.account, dismissed) });
    } catch (e) {
      set({ failed: { ...get().failed, [checkKey(c)]: String(e) } });
    }
  }
}));
