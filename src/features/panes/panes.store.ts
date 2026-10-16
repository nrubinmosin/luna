import { create } from 'zustand';
import { persist } from 'zustand/middleware';
import { MAX_GROUPS, type GroupId } from '../../shared/types';

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

export { MAX_GROUPS, groupLabel } from '../../shared/types';
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

/** Fills in whatever a persisted group is missing, without dropping what it has. */
const reviveGroup = (g: Partial<Group> | undefined): Group => ({
  layout: g?.layout ?? 1,
  boards: { ...blankBoards(), ...(g?.boards ?? {}) },
  splitsByLayout: { ...blankSplits(), ...(g?.splitsByLayout ?? {}) }
});
const reviveGroups = (list: unknown): Group[] => {
  const kept = Array.isArray(list) ? (list as Array<Partial<Group> | undefined>).slice(0, MAX_GROUPS) : [];
  return kept.length ? kept.map(reviveGroup) : [blankGroup()];
};

const boardsEmpty = (g: Partial<Group> | undefined): boolean =>
  Object.values(g?.boards ?? {}).every(slots => (slots ?? []).every(x => !x));

/**
 * A version-1 store always holds four groups, made or not. Of those, every
 * group up to the last one with a chat or a seated pane stays and the empty
 * tail goes: nothing anyone put somewhere is lost, and nobody is left with
 * groups they never asked for. Reads the chats straight from storage, which
 * is already there when this runs.
 */
const trimUnusedTail = (groups: Array<Partial<Group> | undefined>): Array<Partial<Group> | undefined> => {
  const withChats = new Set<number>();
  try {
    const raw = JSON.parse(localStorage.getItem('luna.chats') ?? 'null') as {
      state?: { folders?: Array<{ chats?: Array<{ group?: number }> }> };
    } | null;
    for (const f of raw?.state?.folders ?? []) for (const c of f.chats ?? []) withChats.add(c.group ?? 0);
  } catch {
    // Unreadable chats: keep every group rather than guess.
    return groups;
  }
  let last = 0;
  groups.forEach((g, i) => {
    if (withChats.has(i) || !boardsEmpty(g)) last = i;
  });
  return groups.slice(0, last + 1);
};

/**
 * Each layout's panes in the order auto-tile fills them. Growing a board by one
 * keeps every chat where it was and puts the newcomer in the one new place:
 * 1 → 2 opens the right half, 2 → 3 splits the right column, and 3 → 4 splits
 * the left one — layout 3's right column is slots 1 and 2, layout 4's is 1 and
 * 3, so the old bottom-right chat moves to slot 3 and the new one takes slot 2,
 * bottom-left. Shrinking walks the same order back.
 */
const TILE_ORDER: Record<Layout, number[]> = { 1: [0], 2: [0, 1], 3: [0, 1, 2], 4: [0, 1, 3, 2] };

/** The chats on a group's live board, in tile order. */
const seatedChats = (g: Group): string[] =>
  TILE_ORDER[g.layout].map(i => g.boards[g.layout][i]).filter((x): x is string => !!x);

/** The group re-laid as exactly as many panes as it has chats (one at least). */
const tiled = (g: Group, chats: string[]): Group => {
  const n = Math.min(4, Math.max(1, chats.length)) as Layout;
  const slots = emptySlots();
  chats.slice(0, 4).forEach((id, i) => (slots[TILE_ORDER[n][i]] = id));
  return { ...g, layout: n, boards: { ...g.boards, [n]: slots } };
};

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
  /** The pane last worked in — a new chat takes its folder and account after it. */
  activePane: number;
  /**
   * Auto-tile: a new chat with no free pane to land in grows the board by one
   * pane instead of coming up on the sheet, and a pane emptied by closing or
   * deleting its chat shrinks it by one. Switched in Settings; persisted.
   */
  autoTile: boolean;
  setAutoTile: (on: boolean) => void;
  setGroup: (g: GroupId) => void;
  /** The next group round, for Ctrl+Tab. */
  nextGroup: () => void;
  /** A new empty group at the end, switched to. No-op at MAX_GROUPS. */
  addGroup: () => void;
  /**
   * Takes group `g` off the boards. Whether it may go — no chat in it — is
   * the chats store's call (`removeGroup` there), which renumbers its chats
   * and then calls this.
   */
  dropGroup: (g: GroupId) => void;
  /** Makes sure groups up to index `g` exist, so no chat is ever listed in
   *  a group the title bar does not offer. */
  coverGroup: (g: GroupId) => void;
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
  /** Brings a chat up the way showChat does, minus the toggle and the swap. */
  previewChat: (chatId: string) => void;
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

/** Replaces the active group, leaving the others untouched. */
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

/** Where the pane being worked in went after the board was re-tiled. */
const keepActive = (s: PanesState, next: Group): number => {
  const was = currentSlots(s)[s.activePane];
  return Math.max(0, was ? next.boards[next.layout].indexOf(was) : 0);
};

/** The rest of evicting a chat, once its boards have been dealt with. */
const evictTail = (s: PanesState, boards: { groups: Group[] }, chatId: string) => ({
  ...boards,
  // Same as closePane: a sheet over the pane it just emptied would trap the
  // view with no Return button to leave by.
  ...(s.peek != null && currentSlots(s)[s.peek] === chatId ? { peek: null } : {}),
  // And a sheet showing a chat that has just been deleted has nothing left
  // to show.
  ...(s.peekChat === chatId ? { peekChat: null } : {})
});

export const usePanes = create<PanesState>()(
  persist(
    set => ({
      group: 0,
      groups: [blankGroup()],
      over: -1,
      drag: null,
      dragPane: null,
      spot: null,
      peek: null,
      peekChat: null,
      activePane: 0,
      autoTile: true,

      setAutoTile: on => set({ autoTile: on }),

      // Switching group or layout lands on a different board, where the old
      // pane index would point at an unrelated pane — so both drop the peek and
      // the active pane with it.
      setGroup: g => set({ group: g, over: -1, dragPane: null, activePane: 0, ...NO_PEEK }),

      nextGroup: () =>
        set(s => ({ group: (s.group + 1) % s.groups.length, over: -1, dragPane: null, activePane: 0, ...NO_PEEK })),

      addGroup: () =>
        set(s =>
          s.groups.length >= MAX_GROUPS
            ? {}
            : { groups: [...s.groups, blankGroup()], group: s.groups.length, over: -1, dragPane: null, activePane: 0, ...NO_PEEK }
        ),

      dropGroup: g =>
        set(s => {
          if (s.groups.length <= 1 || g < 0 || g >= s.groups.length) return {};
          const groups = s.groups.filter((_, i) => i !== g);
          // The groups after it shift down by one; the one on screen stays on
          // screen unless it is the one going, which hands over to its left.
          const group = s.group > g ? s.group - 1 : s.group === g ? Math.max(0, g - 1) : s.group;
          return { groups, group, over: -1, dragPane: null, ...(s.group === g ? { activePane: 0, ...NO_PEEK } : {}) };
        }),

      coverGroup: g =>
        set(s => {
          const want = Math.min(MAX_GROUPS, g + 1);
          if (s.groups.length >= want) return {};
          return { groups: [...s.groups, ...Array.from({ length: want - s.groups.length }, blankGroup)] };
        }),

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
          if (s.autoTile) {
            const g = { ...currentGroup(s), boards: { ...currentGroup(s).boards, [currentLayout(s)]: slots } };
            const rest = seatedChats(g);
            if (rest.length < g.layout) {
              const next = tiled(g, rest);
              return {
                groups: s.groups.map((x, i) => (i === s.group ? next : x)),
                activePane: keepActive(s, next),
                ...NO_PEEK
              };
            }
          }
          // An emptied pane has no title bar, so a sheet held up over it would
          // have no Return button left to press — back to the grid instead.
          return { ...withBoard(s, slots), ...(s.peek === paneIndex ? { peek: null } : {}) };
        }),

      // A deleted chat has to disappear from every board of every group, not
      // just the visible one.
      evictChat: chatId =>
        set(s => {
          const cleared = mapAllBoards(s, slots => slots.map(p => (p === chatId ? null : p)));
          if (!s.autoTile) return evictTail(s, cleared, chatId);
          // Auto-tile closes the gap on every group whose live board had the
          // chat, parked ones included — coming back to a group should not
          // mean coming back to a hole.
          const groups = cleared.groups.map((g, i) => {
            if (!s.groups[i].boards[s.groups[i].layout].includes(chatId)) return g;
            const rest = seatedChats(g);
            return rest.length < g.layout ? tiled(g, rest) : g;
          });
          const reshaped = groups[s.group].layout !== s.groups[s.group].layout;
          return reshaped
            ? { groups, activePane: keepActive(s, groups[s.group]), ...NO_PEEK }
            : evictTail(s, { groups }, chatId);
        }),

      // Seat a new chat wherever it will be seen: a free pane of the board on
      // screen if there is one. When there is none — every pane taken, the
      // one-pane layout included — the chat comes up full size as a sheet of
      // its own and the board stays as the user arranged it: taking over the
      // pane last worked in swapped out a chat that was being watched. The
      // other layouts of the group take it only if they have room, and parked
      // groups are left alone: they are parked on purpose.
      autoPlace: chatId =>
        set(s => {
          const g = currentGroup(s);
          const live = currentLayout(s);

          // Auto-tile: a full board with room to grow takes one more pane and
          // seats the chat there, where it would otherwise go up on the sheet.
          const onBoard = g.boards[live].slice(0, live).includes(chatId);
          if (s.autoTile && !onBoard && live < 4 && seatedChats(g).length >= live) {
            const grown = tiled(g, [...seatedChats(g), chatId]);
            return {
              groups: s.groups.map((x, i) => (i === s.group ? grown : x)),
              activePane: grown.boards[grown.layout].indexOf(chatId),
              ...NO_PEEK
            };
          }

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

          // Creating a chat is asking to work in it, now: with no pane to show
          // it in it goes up on the sheet, and a sheet that is already up
          // follows the new chat rather than hiding it.
          const seated = boards[live].indexOf(chatId);
          if (seated < 0 || seated >= live) return { ...withGroup(s, { boards }), peekChat: chatId, peek: null };
          return {
            ...withGroup(s, { boards }),
            activePane: seated,
            ...(peeking(s) ? { peek: seated, peekChat: null } : {})
          };
        }),

      /**
       * One click on a row in the sidebar. With a single pane there is nothing
       * to arrange and dragging is pure ceremony, so the click simply swaps
       * what that pane shows. With more panes the board is an arrangement the
       * user made, and a click must not rewrite it — but it still opens the
       * chat full size, the same as in the one-pane layout: a chat already on
       * the board is held up on the sheet from its own pane, and a chat that
       * is not on it comes up as a sheet of its own. Either way the board is
       * left exactly as it was. Clicking the row of the chat that is already
       * up puts it back, the same toggle Ctrl+<digit> is.
       */
      showChat: chatId =>
        set(s => {
          if (currentLayout(s) === 1) {
            const slots = currentSlots(s).slice();
            slots[0] = chatId;
            return { ...withBoard(s, slots), activePane: 0, ...NO_PEEK };
          }
          const i = currentSlots(s).indexOf(chatId);
          if (i < 0 || i >= currentLayout(s)) {
            return s.peekChat === chatId ? NO_PEEK : { peekChat: chatId, peek: null };
          }
          return s.peek === i ? NO_PEEK : { activePane: i, peek: i, peekChat: null };
        }),

      /**
       * Holding Alt over a row in the list. The same sheet a click would raise,
       * except it never puts a sheet back and never rewrites the one-pane
       * board: a glance must leave everything as it found it once Alt is let
       * go. A chat already in the single pane is on screen as it is.
       */
      previewChat: chatId =>
        set(s => {
          const i = currentSlots(s).indexOf(chatId);
          if (i < 0 || i >= currentLayout(s)) return { peekChat: chatId, peek: null };
          return currentLayout(s) === 1 ? NO_PEEK : { peek: i, peekChat: null };
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
      // 2: groups are as many as the user made, not a fixed four.
      version: 2,
      partialize: s => ({ group: s.group, groups: s.groups, autoTile: s.autoTile }),
      // What is stored may be short of fields; `merge` fills them in.
      migrate: (persisted, version) => {
        const p = (persisted ?? {}) as Pick<PanesState, 'group' | 'groups' | 'autoTile'>;
        return version < 2 && Array.isArray(p.groups) ? { ...p, groups: trimUnusedTail(p.groups) as Group[] } : p;
      },
      merge: (persisted, current) => {
        const p = (persisted ?? {}) as Partial<PanesState>;
        const groups = reviveGroups(p.groups);
        const group = typeof p.group === 'number' && p.group < groups.length ? p.group : groups.length - 1;
        return { ...current, ...p, group, groups, autoTile: p.autoTile ?? true };
      }
    }
  )
);
