import { create } from 'zustand';
import { persist } from 'zustand/middleware';

export type Layout = 1 | 2 | 3 | 4;
export const LAYOUTS: Layout[] = [1, 2, 3, 4];

/** Fractions of the grid taken by the first column / the top row of each column. */
export interface Splits {
  col: number;
  rowL: number;
  rowR: number;
}
export type SplitKey = keyof Splits;

export type Slots = (string | null)[];
type PerLayout<T> = Record<Layout, T>;

const DEFAULT_SPLITS: Splits = { col: 0.5, rowL: 0.5, rowR: 0.5 };
export const SPLIT_MIN = 0.15;
export const SPLIT_MAX = 0.85;

const emptySlots = (): Slots => [null, null, null, null];
const blankBoards = (): PerLayout<Slots> => ({ 1: emptySlots(), 2: emptySlots(), 3: emptySlots(), 4: emptySlots() });
const blankSplits = (): PerLayout<Splits> => ({
  1: { ...DEFAULT_SPLITS },
  2: { ...DEFAULT_SPLITS },
  3: { ...DEFAULT_SPLITS },
  4: { ...DEFAULT_SPLITS }
});

interface PanesState {
  layout: Layout;
  /**
   * Each layout is its own board: switching 4 → 1 → 4 must bring back exactly
   * the chats that were on the four-pane board, so nothing is cleared on
   * switch and every layout keeps its own slots and its own split fractions.
   */
  boards: PerLayout<Slots>;
  splitsByLayout: PerLayout<Splits>;
  over: number;
  drag: string | null;
  /** Chat hovered in the sidebar — its pane lights up so you can spot it. */
  spot: string | null;
  setLayout: (n: Layout) => void;
  setSplit: (key: SplitKey, value: number) => void;
  resetSplits: () => void;
  dropChat: (paneIndex: number, chatId: string) => void;
  closePane: (paneIndex: number) => void;
  evictChat: (chatId: string) => void;
  autoPlace: (chatId: string) => void;
  setOver: (i: number) => void;
  setDrag: (id: string | null) => void;
  setSpot: (id: string | null) => void;
}

/** Slots of the board currently on screen. */
export const currentSlots = (s: PanesState): Slots => s.boards[s.layout];
export const currentSplits = (s: PanesState): Splits => s.splitsByLayout[s.layout];

/** Replaces only the active board, leaving the other three untouched. */
const withBoard = (s: PanesState, next: Slots) => ({
  boards: { ...s.boards, [s.layout]: next }
});

export const usePanes = create<PanesState>()(
  persist(
    set => ({
      layout: 1,
      boards: blankBoards(),
      splitsByLayout: blankSplits(),
      over: -1,
      drag: null,
      spot: null,

      setLayout: n => set({ layout: n, over: -1 }),

      dropChat: (paneIndex, chatId) =>
        set(s => {
          const slots = currentSlots(s).map(p => (p === chatId ? null : p));
          slots[paneIndex] = chatId;
          return { ...withBoard(s, slots), over: -1, drag: null };
        }),

      closePane: paneIndex =>
        set(s => {
          const slots = currentSlots(s).slice();
          slots[paneIndex] = null;
          return withBoard(s, slots);
        }),

      // A deleted chat has to disappear from every board, not just the visible one.
      evictChat: chatId =>
        set(s => ({
          boards: LAYOUTS.reduce((acc, n) => {
            acc[n] = s.boards[n].map(p => (p === chatId ? null : p));
            return acc;
          }, {} as PerLayout<Slots>)
        })),

      // Seat a new chat on every board that still has room, so switching layout
      // right after creating it doesn't land on an empty grid.
      autoPlace: chatId =>
        set(s => ({
          boards: LAYOUTS.reduce((acc, n) => {
            const slots = s.boards[n];
            if (slots.includes(chatId)) {
              acc[n] = slots;
              return acc;
            }
            const free = slots.findIndex((x, i) => i < n && !x);
            if (free < 0) {
              acc[n] = slots;
              return acc;
            }
            const next = slots.slice();
            next[free] = chatId;
            acc[n] = next;
            return acc;
          }, {} as PerLayout<Slots>)
        })),

      setOver: i => set({ over: i }),
      setDrag: id => set({ drag: id }),
      setSpot: id => set({ spot: id }),

      setSplit: (key, value) =>
        set(s => ({
          splitsByLayout: {
            ...s.splitsByLayout,
            [s.layout]: {
              ...currentSplits(s),
              [key]: Math.min(SPLIT_MAX, Math.max(SPLIT_MIN, value))
            }
          }
        })),

      resetSplits: () =>
        set(s => ({
          splitsByLayout: { ...s.splitsByLayout, [s.layout]: { ...DEFAULT_SPLITS } }
        }))
    }),
    {
      name: 'llm-desktop.panes',
      version: 1,
      partialize: s => ({ layout: s.layout, boards: s.boards, splitsByLayout: s.splitsByLayout }),
      // v0 kept a single shared `panes` array and one `splits` object. Seed every
      // board from it so an upgrade doesn't wipe the user's current arrangement.
      migrate: (persisted, version) => {
        type Stored = { layout: Layout; boards: PerLayout<Slots>; splitsByLayout: PerLayout<Splits> };
        if (version >= 1) {
          const p = (persisted ?? {}) as Partial<Stored>;
          return {
            layout: p.layout ?? 1,
            boards: { ...blankBoards(), ...(p.boards ?? {}) },
            splitsByLayout: { ...blankSplits(), ...(p.splitsByLayout ?? {}) }
          };
        }
        const old = persisted as { layout?: Layout; panes?: Slots; splits?: Partial<Splits> } | undefined;
        const seed = old?.panes ?? emptySlots();
        const splits = { ...DEFAULT_SPLITS, ...(old?.splits ?? {}) };
        return {
          layout: old?.layout ?? 1,
          boards: LAYOUTS.reduce((acc, n) => {
            // Slots past a board's pane count were never visible on it.
            acc[n] = seed.map((id, i) => (i < n ? id : null));
            return acc;
          }, {} as PerLayout<Slots>),
          splitsByLayout: LAYOUTS.reduce((acc, n) => {
            acc[n] = { ...splits };
            return acc;
          }, {} as PerLayout<Splits>)
        };
      },
      merge: (persisted, current) => {
        const p = (persisted ?? {}) as Partial<PanesState>;
        return {
          ...current,
          ...p,
          boards: { ...blankBoards(), ...(p.boards ?? {}) },
          splitsByLayout: { ...blankSplits(), ...(p.splitsByLayout ?? {}) }
        };
      }
    }
  )
);
