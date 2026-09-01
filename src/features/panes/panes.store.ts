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
  /**
   * Pane held up over the board as a temporary sheet. The grid stays mounted
   * and visible behind it, only dimmed, which is the whole point: a peek has
   * to read as a way of looking and not as an arrangement. The board, its
   * slots and its splits are untouched, and none of this is persisted.
   */
  peek: number | null;
  /**
   * The same sheet for a chat with no seat on the board — clicking a chat that
   * is not in any pane shows it without disturbing what is. At most one of
   * `peek` / `peekChat` is set at a time.
   */
  peekChat: string | null;
  /**
   * The pane last worked in. Nothing but placement reads it: it is where a new
   * chat lands when the board has no free pane left.
   */
  activePane: number;
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
  /** One click in the sidebar — see the comment on the implementation. */
  showChat: (chatId: string) => void;
  setOver: (i: number) => void;
  setDrag: (id: string | null) => void;
  setDragPane: (i: number | null) => void;
  /** Exchanges the contents of two panes on the active board. */
  swapPanes: (a: number, b: number) => void;
  setSpot: (id: string | null) => void;
  setActivePane: (i: number) => void;
  setPeek: (i: number | null) => void;
  setPeekChat: (id: string | null) => void;
  closePeek: () => void;
}

/** The group currently on screen. */
export const currentGroup = (s: PanesState): Group => s.groups[s.group] ?? s.groups[0];
export const currentLayout = (s: PanesState): Layout => currentGroup(s).layout;
/** Slots of the board currently on screen. */
export const currentSlots = (s: PanesState): Slots => currentGroup(s).boards[currentLayout(s)];
export const currentSplits = (s: PanesState): Splits => currentGroup(s).splitsByLayout[currentLayout(s)];
/** Whether a sheet is up over the board, of either kind. */
export const peeking = (s: PanesState): boolean => s.peek != null || s.peekChat != null;

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

const NO_PEEK = { peek: null, peekChat: null } as const;

export const usePanes = create<PanesState>()(
  persist(
    set => ({
      group: 0,
      groups: blankGroups(),
      over: -1,
      drag: null,
      dragPane: null,
      spot: null,
      peek: null,
      peekChat: null,
      activePane: 0,

      // Switching group or layout lands on a different board, where the old
      // pane index would point at an unrelated pane — so both drop the peek and
      // the active pane with it.
      setGroup: g => set({ group: g, over: -1, dragPane: null, activePane: 0, ...NO_PEEK }),

      resetGroup: g =>
        set(s => ({
          groups: s.groups.map((old, i) => (i === g ? { ...blankGroup(), layout: old.layout } : old)),
          over: -1,
          // Resetting the visible group empties the pane under the sheet too,
          // and an empty pane offers no way back out of the peek.
          ...(g === s.group ? { activePane: 0, ...NO_PEEK } : {})
        })),

      setLayout: n => set(s => ({ ...withGroup(s, { layout: n }), over: -1, activePane: 0, ...NO_PEEK })),

      dropChat: (paneIndex, chatId) =>
        set(s => {
          const slots = currentSlots(s).map(p => (p === chatId ? null : p));
          slots[paneIndex] = chatId;
          return { ...withBoard(s, slots), over: -1, drag: null, activePane: paneIndex };
        }),

      closePane: paneIndex =>
        set(s => {
          const slots = currentSlots(s).slice();
          slots[paneIndex] = null;
          // An emptied pane has no title bar, so a sheet held up over it would
          // have no Return button left to press — back to the grid instead.
          return { ...withBoard(s, slots), ...(s.peek === paneIndex ? { peek: null } : {}) };
        }),

      // A deleted chat has to disappear from every board of every group, not
      // just the visible one.
      evictChat: chatId =>
        set(s => ({
          ...mapAllBoards(s, slots => slots.map(p => (p === chatId ? null : p))),
          // Same as closePane: a sheet over the pane it just emptied would trap
          // the view with no Return button to leave by.
          ...(s.peek != null && currentSlots(s)[s.peek] === chatId ? { peek: null } : {}),
          // And a sheet showing a chat that has just been deleted has nothing
          // left to show.
          ...(s.peekChat === chatId ? { peekChat: null } : {})
        })),

      // Seat a new chat wherever it will be seen. On the board that is on
      // screen that means a free pane if there is one and the pane last worked
      // in otherwise — a new chat you cannot see is one you have to go hunting
      // for, which in the one-pane layout used to be every new chat. The other
      // layouts of the group take it only if they have room, and parked groups
      // are left alone: they are parked on purpose.
      autoPlace: chatId =>
        set(s => {
          const g = currentGroup(s);
          const live = currentLayout(s);
          const boards = LAYOUTS.reduce((acc, n) => {
            const slots = g.boards[n];
            if (slots.includes(chatId)) {
              acc[n] = slots;
              return acc;
            }
            const free = slots.findIndex((x, i) => i < n && !x);
            const seat = free >= 0 ? free : n === live ? Math.min(s.activePane, n - 1) : -1;
            if (seat < 0) {
              acc[n] = slots;
              return acc;
            }
            const next = slots.slice();
            next[seat] = chatId;
            acc[n] = next;
            return acc;
          }, {} as PerLayout<Slots>);

          // A sheet that is already up follows the new chat rather than hiding
          // it: creating a chat is asking to work in it, now.
          const seated = boards[live].indexOf(chatId);
          return {
            ...withGroup(s, { boards }),
            activePane: seated >= 0 ? seated : s.activePane,
            ...(peeking(s) ? { peek: seated >= 0 ? seated : null, peekChat: null } : {})
          };
        }),

      /**
       * One click on a row in the sidebar. With a single pane there is nothing
       * to arrange and dragging is pure ceremony, so the click simply swaps
       * what that pane shows. With more panes the board is an arrangement the
       * user made, and a click must not rewrite it: a chat already on it just
       * becomes the active pane (or, while a sheet is up, the one on the
       * sheet), and a chat that is not on it comes up as its own sheet, which
       * leaves the board exactly as it was.
       */
      showChat: chatId =>
        set(s => {
          if (currentLayout(s) === 1) {
            const slots = currentSlots(s).slice();
            slots[0] = chatId;
            return { ...withBoard(s, slots), activePane: 0, ...NO_PEEK };
          }
          const i = currentSlots(s).indexOf(chatId);
          if (i < 0 || i >= currentLayout(s)) return { peekChat: chatId, peek: null };
          return { activePane: i, ...(peeking(s) ? { peek: i, peekChat: null } : {}) };
        }),

      setOver: i => set({ over: i }),
      setDrag: id => set({ drag: id }),
      setDragPane: i => set({ dragPane: i }),

      swapPanes: (a, b) =>
        set(s => {
          if (a === b) return { over: -1, dragPane: null };
          const slots = currentSlots(s).slice();
          [slots[a], slots[b]] = [slots[b], slots[a]];
          return { ...withBoard(s, slots), over: -1, dragPane: null, activePane: b };
        }),

      setSpot: id => set({ spot: id }),
      setActivePane: i => set({ activePane: i }),
      // Only one thing can be up on the sheet, so each kind replaces the other.
      setPeek: i => set(i == null ? { peek: null } : { peek: i, peekChat: null, activePane: i }),
      setPeekChat: id => set({ peekChat: id, peek: null }),
      closePeek: () => set(NO_PEEK),

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
