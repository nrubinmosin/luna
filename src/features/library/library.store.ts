import { create } from 'zustand';
import { libraryOverview, librarySync, type LibraryOverviewDto, type PlaceDto } from '../../ipc/commands';

/** A copy that waits on the user: changed or deleted there, someone else's
 *  file in its place, or one that cannot be laid out at all. */
export const needsLook = (p: PlaceDto) =>
  p.status === 'changed' || p.status === 'gone' || p.status === 'taken' || p.status === 'failed';

/** The copies that wait on the user, a line each — except those that fail for
 *  one reason (a skill that does not do, wherever it goes), which share one. */
export function waiting(places: PlaceDto[]): { place: PlaceDto; count: number }[] {
  const out: { place: PlaceDto; count: number }[] = [];
  const failed = new Map<string, { place: PlaceDto; count: number }>();
  for (const place of places.filter(needsLook)) {
    if (place.status !== 'failed') {
      out.push({ place, count: 1 });
      continue;
    }
    const key = `${place.skill}|${place.problem}`;
    const seen = failed.get(key);
    if (seen) seen.count++;
    else {
      const line = { place, count: 1 };
      failed.set(key, line);
      out.push(line);
    }
  }
  return out;
}

/** How long a refresh on window focus waits after the last look. */
const FOCUS_GAP_MS = 15_000;

interface LibraryState {
  open: boolean;
  setOpen: (v: boolean) => void;
  overview: LibraryOverviewDto | null;
  error: string | null;
  /** When the overview was last read. */
  at: number;
  /** Lays the library out once at startup, then looks again whenever the
   *  window comes back: a copy edited in another program shows up then. */
  init: () => void;
  /** Where every copy stands, writing nothing. */
  refresh: () => Promise<void>;
  /** Runs a command that answers with the overview and keeps that; false when it failed. */
  run: (p: Promise<LibraryOverviewDto | null>) => Promise<boolean>;
}

let started = false;
// Commands answer out of order — a focus refresh asked before a click can land
// after it — so an answer older than the one on screen is dropped.
let asked = 0;
let shown = 0;

export const useLibrary = create<LibraryState>((set, get) => ({
  open: false,
  setOpen: v => {
    set({ open: v });
    if (v) void get().refresh();
  },
  overview: null,
  error: null,
  at: 0,
  init: () => {
    if (started) return;
    started = true;
    void get().run(librarySync());
    window.addEventListener('focus', () => {
      if (Date.now() - get().at > FOCUS_GAP_MS) void get().refresh();
    });
  },
  refresh: async () => {
    await get().run(libraryOverview());
  },
  run: async p => {
    const mine = ++asked;
    try {
      const overview = await p;
      if (mine > shown) {
        shown = mine;
        set({ overview, error: null, at: Date.now() });
      }
      return true;
    } catch (e) {
      set({ error: String(e), at: Date.now() });
      return false;
    }
  }
}));
