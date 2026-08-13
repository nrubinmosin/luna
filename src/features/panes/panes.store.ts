import { create } from 'zustand';
import { persist } from 'zustand/middleware';
import { GROUPS, type GroupId } from '../../shared/types';

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

export { GROUPS, GROUP_LABELS } from '../../shared/types';
export type { GroupId } from '../../shared/types';

/**
 * One remembered workspace: which layout it was left on, what sat in the panes
 * of each of its four boards, and how those boards were split. Groups are the
 * level above layouts — switching group swaps the whole arrangement at once,
 * so a set of chats can be parked and brought back untouched.
 */
export interface Group {
  layout: Layout;
  boards: PerLayout<Slots>;
  splitsByLayout: PerLayout<Splits>;
}

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
const blankGroup = (): Group => ({ layout: 1, boards: blankBoards(), splitsByLayout: blankSplits() });
const blankGroups = (): Group[] => GROUPS.map(blankGroup);

/** Fills in whatever a persisted group is missing, without dropping what it has. */
const reviveGroup = (g: Partial<Group> | undefined): Group => ({
  layout: g?.layout ?? 1,
  boards: { ...blankBoards(), ...(g?.boards ?? {}) },
  splitsByLayout: { ...blankSplits(), ...(g?.splitsByLayout ?? {}) }
});
const reviveGroups = (list: unknown): Group[] =>
  GROUPS.map(i => reviveGroup(Array.isArray(list) ? (list[i] as Partial<Group> | undefined) : undefined));

interface PanesState {
  group: GroupId;
  /**
   * Each group is its own remembered set of boards, and inside a group each
   * layout is its own board: switching 4 → 1 → 4, or II → I → II, must bring
   * back exactly what was there, so nothing is ever cleared on a switch.
   */
  groups: Group[];
  over: number;
  drag: string | null;
  /** Index of the pane being dragged by its header, for swapping two panes. */
  dragPane: number | null;
  /** Chat hovered in the sidebar — its pane lights up so you can spot it. */
  spot: string | null;
  setGroup: (g: GroupId) => void;
  /** Empties one group's boards and evens out its splits. Chats are untouched. */
  resetGroup: (g: GroupId) => void;
  setLayout: (n: Layout) => void;
  setSplit: (key: SplitKey, value: number) => void;
  resetSplits: () => void;
  dropChat: (paneIndex: number, chatId: string) => void;
  closePane: (paneIndex: number) => void;
  evictChat: (chatId: string) => void;
  autoPlace: (chatId: string) => void;
  setOver: (i: number) => void;
  setDrag: (id: string | null) => void;
  setDragPane: (i: number | null) => void;
  /** Exchanges the contents of two panes on the active board. */
  swapPanes: (a: number, b: number) => void;
  setSpot: (id: string | null) => void;
}

/** The group currently on screen. */
export const currentGroup = (s: PanesState): Group => s.groups[s.group] ?? s.groups[0];
export const currentLayout = (s: PanesState): Layout => currentGroup(s).layout;
/** Slots of the board currently on screen. */
export const currentSlots = (s: PanesState): Slots => currentGroup(s).boards[currentLayout(s)];
export const currentSplits = (s: PanesState): Splits => currentGroup(s).splitsByLayout[currentLayout(s)];

/** Replaces the active group, leaving the other three untouched. */
const withGroup = (s: PanesState, patch: Partial<Group>) => ({
  groups: s.groups.map((g, i) => (i === s.group ? { ...g, ...patch } : g))
});

/** Replaces only the active board, leaving the other three layouts untouched. */
const withBoard = (s: PanesState, next: Slots) =>
  withGroup(s, { boards: { ...currentGroup(s).boards, [currentLayout(s)]: next } });

/** Applies a slot rewrite to every board of every group. */
const mapAllBoards = (s: PanesState, fn: (slots: Slots, layout: Layout) => Slots) => ({
  groups: s.groups.map(g => ({
    ...g,
    boards: LAYOUTS.reduce((acc, n) => {
      acc[n] = fn(g.boards[n], n);
      return acc;
    }, {} as PerLayout<Slots>)
  }))
});

export const usePanes = create<PanesState>()(
  persist(
    set => ({
      group: 0,
      groups: blankGroups(),
      over: -1,
      drag: null,
      dragPane: null,
      spot: null,

      setGroup: g => set({ group: g, over: -1, dragPane: null }),

      resetGroup: g =>
        set(s => ({
          groups: s.groups.map((old, i) => (i === g ? { ...blankGroup(), layout: old.layout } : old)),
          over: -1
        })),

      setLayout: n => set(s => ({ ...withGroup(s, { layout: n }), over: -1 })),

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

      // A deleted chat has to disappear from every board of every group, not
      // just the visible one.
      evictChat: chatId => set(s => mapAllBoards(s, slots => slots.map(p => (p === chatId ? null : p)))),

      // Seat a new chat on every board of the active group that still has room,
      // so switching layout right after creating it doesn't land on an empty
      // grid. Parked groups are left alone: they are parked on purpose.
      autoPlace: chatId =>
        set(s => {
          const g = currentGroup(s);
          const boards = LAYOUTS.reduce((acc, n) => {
            const slots = g.boards[n];
            const free = slots.findIndex((x, i) => i < n && !x);
            if (slots.includes(chatId) || free < 0) {
              acc[n] = slots;
              return acc;
            }
            const next = slots.slice();
            next[free] = chatId;
            acc[n] = next;
            return acc;
          }, {} as PerLayout<Slots>);
          return withGroup(s, { boards });
        }),

      setOver: i => set({ over: i }),
      setDrag: id => set({ drag: id }),
      setDragPane: i => set({ dragPane: i }),

      swapPanes: (a, b) =>
        set(s => {
          if (a === b) return { over: -1, dragPane: null };
          const slots = currentSlots(s).slice();
          [slots[a], slots[b]] = [slots[b], slots[a]];
          return { ...withBoard(s, slots), over: -1, dragPane: null };
        }),

      setSpot: id => set({ spot: id }),

      setSplit: (key, value) =>
        set(s =>
          withGroup(s, {
            splitsByLayout: {
              ...currentGroup(s).splitsByLayout,
              [currentLayout(s)]: {
                ...currentSplits(s),
                [key]: Math.min(SPLIT_MAX, Math.max(SPLIT_MIN, value))
              }
            }
          })
        ),

      resetSplits: () =>
        set(s =>
          withGroup(s, {
            splitsByLayout: { ...currentGroup(s).splitsByLayout, [currentLayout(s)]: { ...DEFAULT_SPLITS } }
          })
        )
    }),
    {
      name: 'luna.panes',
      version: 1,
      partialize: s => ({ group: s.group, groups: s.groups }),
      merge: (persisted, current) => {
        const p = (persisted ?? {}) as Partial<PanesState>;
        return { ...current, ...p, group: p.group ?? 0, groups: reviveGroups(p.groups) };
      }
    }
  )
);
