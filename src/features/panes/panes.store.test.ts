import { describe, expect, it } from 'vitest';
import { MAX_GROUPS } from '../../shared/types';
import {
  currentGroup,
  currentLayout,
  currentSlots,
  currentSplits,
  peeking,
  SPLIT_MAX,
  SPLIT_MIN,
  usePanes,
  type Layout
} from './panes.store';

const panes = () => usePanes.getState();
const layout = () => currentLayout(panes());
/** The panes on screen, as many as the layout shows. */
const board = () => currentSlots(panes()).slice(0, layout());

/** Seats chats into the panes of `n`, left to right in slot order, with auto-tile off. */
function arrange(n: Layout, chats: Array<string | null>) {
  usePanes.setState({ autoTile: false });
  panes().setLayout(n);
  chats.forEach((id, i) => id && panes().dropChat(i, id));
}

/** Opens chats one after another the way creating them does, with auto-tile on. */
function openChats(...ids: string[]) {
  usePanes.setState({ autoTile: true });
  ids.forEach(id => panes().autoPlace(id));
}

describe('panes store: layouts', () => {
  it('starts on one empty pane in a single group, with auto-tile on', () => {
    expect(panes().groups).toHaveLength(1);
    expect(layout()).toBe(1);
    expect(board()).toEqual([null]);
    expect(panes().autoTile).toBe(true);
  });

  it('keeps each layout as its own board, so switching away and back restores it', () => {
    arrange(4, ['a', 'b', 'c', 'd']);
    panes().setLayout(1);
    expect(board()).toEqual([null]);
    panes().dropChat(0, 'x');
    panes().setLayout(4);
    expect(board()).toEqual(['a', 'b', 'c', 'd']);
    panes().setLayout(1);
    expect(board()).toEqual(['x']);
  });

  it('drops the sheet and the active pane on a layout switch', () => {
    arrange(2, ['a', 'b']);
    panes().setPeek(1);
    panes().setLayout(3);
    expect(peeking(panes())).toBe(false);
    expect(panes().activePane).toBe(0);
  });

  it('moves a chat dropped on another pane rather than showing it twice', () => {
    arrange(2, ['a', null]);
    panes().setDrag('a');
    panes().dropChat(1, 'a');
    expect(board()).toEqual([null, 'a']);
    expect(panes().activePane).toBe(1);
    expect(panes().drag).toBeNull();
  });
});

describe('panes store: placing a new chat with auto-tile off', () => {
  it('seats it in the first free pane and makes that pane active', () => {
    arrange(3, ['a', null, null]);
    panes().autoPlace('b');
    expect(board()).toEqual(['a', 'b', null]);
    expect(panes().activePane).toBe(1);
  });

  it('raises it on the sheet when every pane is taken, leaving the board as it was', () => {
    arrange(2, ['a', 'b']);
    panes().autoPlace('c');
    expect(board()).toEqual(['a', 'b']);
    expect(panes().peekChat).toBe('c');
  });

  it('also seats it on the group’s other layouts that have room', () => {
    arrange(1, ['a']);
    panes().autoPlace('b');
    panes().setLayout(2);
    expect(board()).toEqual(['b', null]);
  });

  it('moves a sheet already up over to the new chat', () => {
    arrange(2, ['a', null]);
    panes().setPeek(0);
    panes().autoPlace('b');
    expect(panes().peek).toBe(1);
    expect(panes().peekChat).toBeNull();
  });
});

describe('panes store: auto-tile', () => {
  it('grows the board one pane per new chat, keeping every chat where it was', () => {
    openChats('a');
    expect(board()).toEqual(['a']);
    openChats('b');
    expect(board()).toEqual(['a', 'b']);
    openChats('c');
    expect(board()).toEqual(['a', 'b', 'c']);
  });

  it('going from three panes to four, puts the newcomer bottom-left and the old bottom-right chat bottom-right', () => {
    openChats('a', 'b', 'c', 'd');
    expect(layout()).toBe(4);
    // Layout 4's slots read top-left, top-right, bottom-left, bottom-right.
    expect(board()).toEqual(['a', 'b', 'd', 'c']);
    expect(panes().activePane).toBe(2);
  });

  it('raises a fifth chat on the sheet, as four panes is the most there are', () => {
    openChats('a', 'b', 'c', 'd', 'e');
    expect(layout()).toBe(4);
    expect(board()).not.toContain('e');
    expect(panes().peekChat).toBe('e');
  });

  it('does not grow for a chat that is already on the board', () => {
    openChats('a', 'b');
    panes().autoPlace('a');
    expect(layout()).toBe(2);
    expect(panes().activePane).toBe(0);
  });

  it('fills a free pane before growing', () => {
    arrange(3, ['a', null, 'c']);
    openChats('b');
    expect(layout()).toBe(3);
    expect(board()).toEqual(['a', 'b', 'c']);
  });

  it('shrinks the board by one when a pane is closed, in tile order', () => {
    openChats('a', 'b', 'c', 'd');
    // [a b / d c]; closing the top-right pane walks the 3 → 4 split back:
    // the right column becomes c over d.
    panes().closePane(1);
    expect(board()).toEqual(['a', 'c', 'd']);
  });

  it('keeps the pane being worked in active across the re-tile', () => {
    openChats('a', 'b', 'c');
    panes().setActivePane(2);
    panes().closePane(0);
    expect(board()).toEqual(['b', 'c']);
    expect(currentSlots(panes())[panes().activePane]).toBe('c');
  });

  it('leaves one empty pane when the last chat is closed', () => {
    openChats('a');
    panes().closePane(0);
    expect(layout()).toBe(1);
    expect(board()).toEqual([null]);
  });
});

describe('panes store: closing a pane with auto-tile off', () => {
  it('leaves a hole where the chat was', () => {
    arrange(3, ['a', 'b', 'c']);
    panes().closePane(1);
    expect(board()).toEqual(['a', null, 'c']);
  });

  it('puts down a sheet held over the pane it empties', () => {
    arrange(2, ['a', 'b']);
    panes().setPeek(1);
    panes().closePane(1);
    expect(panes().peek).toBeNull();
  });

  it('keeps a sheet held over another pane', () => {
    arrange(2, ['a', 'b']);
    panes().setPeek(0);
    panes().closePane(1);
    expect(panes().peek).toBe(0);
  });
});

describe('panes store: evicting a deleted chat', () => {
  it('clears it from every layout of every group', () => {
    arrange(2, ['a', 'x']);
    panes().setLayout(1);
    panes().dropChat(0, 'x');
    panes().addGroup();
    arrange(3, ['x', null, null]);
    panes().evictChat('x');
    const everywhere = panes().groups.flatMap(g => Object.values(g.boards).flat());
    expect(everywhere).not.toContain('x');
  });

  it('closes the gap on a parked group too when auto-tile is on', () => {
    openChats('a', 'x', 'b');
    panes().addGroup();
    panes().evictChat('x');
    const parked = panes().groups[0]!;
    expect(parked.layout).toBe(2);
    expect(parked.boards[2].slice(0, 2)).toEqual(['a', 'b']);
  });

  it('leaves alone a group whose live board did not have it', () => {
    arrange(2, ['a', 'b']);
    panes().setLayout(1);
    panes().dropChat(0, 'c');
    usePanes.setState({ autoTile: true });
    // 'a' sits only on layout 2, which is not the board on screen.
    panes().evictChat('a');
    expect(layout()).toBe(1);
    expect(board()).toEqual(['c']);
  });

  it('puts down the sheet showing it', () => {
    arrange(1, ['a']);
    panes().setPeekChat('x');
    panes().evictChat('x');
    expect(panes().peekChat).toBeNull();
  });

  it('puts down a sheet held over its pane', () => {
    arrange(2, ['a', 'x']);
    panes().setPeek(1);
    panes().evictChat('x');
    expect(panes().peek).toBeNull();
  });

  it('keeps a sheet held over another chat’s pane when the board does not move', () => {
    arrange(3, ['a', 'b', 'x']);
    panes().setPeek(0);
    panes().evictChat('x');
    expect(panes().peek).toBe(0);
  });

  it('follows the active chat to its new pane when auto-tile shrinks the board', () => {
    openChats('x', 'a', 'b');
    panes().setActivePane(2);
    panes().evictChat('x');
    expect(currentSlots(panes())[panes().activePane]).toBe('b');
  });
});

describe('panes store: swapping panes', () => {
  it('exchanges what two panes show and makes the target active', () => {
    arrange(3, ['a', 'b', 'c']);
    panes().setDragPane(0);
    panes().swapPanes(0, 2);
    expect(board()).toEqual(['c', 'b', 'a']);
    expect(panes().activePane).toBe(2);
    expect(panes().dragPane).toBeNull();
  });

  it('moves a chat into an empty pane', () => {
    arrange(2, ['a', null]);
    panes().swapPanes(0, 1);
    expect(board()).toEqual([null, 'a']);
  });

  it('changes nothing when a pane is dropped on itself', () => {
    arrange(2, ['a', 'b']);
    panes().setActivePane(1);
    panes().setDragPane(0);
    panes().swapPanes(0, 0);
    expect(board()).toEqual(['a', 'b']);
    expect(panes().activePane).toBe(1);
    expect(panes().dragPane).toBeNull();
  });

  it('touches only the board on screen', () => {
    arrange(2, ['a', 'b']);
    panes().setLayout(3);
    panes().swapPanes(0, 1);
    panes().setLayout(2);
    expect(board()).toEqual(['a', 'b']);
  });
});

describe('panes store: a click on a chat in the sidebar', () => {
  it('swaps what the single pane shows', () => {
    arrange(1, ['a']);
    panes().showChat('b');
    expect(board()).toEqual(['b']);
  });

  it('holds a chat on the board up on the sheet, and a second click puts it back', () => {
    arrange(2, ['a', 'b']);
    panes().showChat('b');
    expect(panes().peek).toBe(1);
    expect(panes().activePane).toBe(1);
    panes().showChat('b');
    expect(peeking(panes())).toBe(false);
    expect(board()).toEqual(['a', 'b']);
  });

  it('raises a chat with no pane on a sheet of its own, leaving the board alone', () => {
    arrange(2, ['a', 'b']);
    panes().showChat('c');
    expect(panes().peekChat).toBe('c');
    expect(board()).toEqual(['a', 'b']);
  });

  it('never puts a sheet back on a glance', () => {
    arrange(2, ['a', 'b']);
    panes().showChat('c');
    panes().previewChat('c');
    expect(panes().peekChat).toBe('c');
  });
});

describe('panes store: groups', () => {
  it('adds a group and switches to it, up to the limit', () => {
    for (let i = 1; i < MAX_GROUPS + 2; i++) panes().addGroup();
    expect(panes().groups).toHaveLength(MAX_GROUPS);
    expect(panes().group).toBe(MAX_GROUPS - 1);
  });

  it('walks the groups round with nextGroup', () => {
    panes().addGroup();
    panes().addGroup();
    expect(panes().group).toBe(2);
    panes().nextGroup();
    expect(panes().group).toBe(0);
  });

  it('keeps each group’s boards apart', () => {
    arrange(2, ['a', 'b']);
    panes().addGroup();
    expect(board()).toEqual([null]);
    panes().setGroup(0);
    expect(board()).toEqual(['a', 'b']);
  });

  it('keeps the group on screen when an earlier one is dropped', () => {
    panes().addGroup();
    arrange(2, ['a', 'b']);
    panes().dropGroup(0);
    expect(panes().group).toBe(0);
    expect(board()).toEqual(['a', 'b']);
  });

  it('hands over to the group on its left when the one on screen is dropped', () => {
    arrange(2, ['a', 'b']);
    panes().addGroup();
    panes().addGroup();
    panes().dropGroup(2);
    expect(panes().group).toBe(1);
  });

  it('never drops the last group', () => {
    panes().dropGroup(0);
    expect(panes().groups).toHaveLength(1);
  });

  it('makes groups up to the one a chat names, and no further than the limit', () => {
    panes().coverGroup(3);
    expect(panes().groups).toHaveLength(4);
    panes().coverGroup(MAX_GROUPS + 5);
    expect(panes().groups).toHaveLength(MAX_GROUPS);
  });

  it('empties a group on reset but keeps its layout', () => {
    arrange(3, ['a', 'b', 'c']);
    panes().setSplit('col', 0.3);
    panes().resetGroup(0);
    expect(layout()).toBe(3);
    expect(board()).toEqual([null, null, null]);
    expect(currentSplits(panes()).col).toBe(0.5);
  });
});

describe('panes store: splits', () => {
  it('clamps a split to the allowed range', () => {
    panes().setLayout(2);
    panes().setSplit('col', 0.01);
    expect(currentSplits(panes()).col).toBe(SPLIT_MIN);
    panes().setSplit('col', 0.99);
    expect(currentSplits(panes()).col).toBe(SPLIT_MAX);
  });

  it('keeps splits per layout', () => {
    panes().setLayout(2);
    panes().setSplit('col', 0.3);
    panes().setLayout(4);
    expect(currentSplits(panes()).col).toBe(0.5);
    panes().setLayout(2);
    expect(currentSplits(panes()).col).toBe(0.3);
  });
});

describe('panes store: persistence', () => {
  const store = (state: unknown, version: number) =>
    localStorage.setItem('luna.panes', JSON.stringify({ state, version }));
  const chatsIn = (...groups: number[]) =>
    localStorage.setItem(
      'luna.chats',
      JSON.stringify({ state: { folders: [{ chats: groups.map(group => ({ group })) }] } })
    );
  const emptyGroup = { layout: 1, boards: { 1: [null, null, null, null] } };
  const seated = (id: string) => ({ layout: 1, boards: { 1: [id, null, null, null] } });

  it('stores the arrangement but not the sheet or the drag', () => {
    arrange(2, ['a', 'b']);
    panes().setPeek(1);
    const saved = JSON.parse(localStorage.getItem('luna.panes')!) as { state: Record<string, unknown> };
    expect(Object.keys(saved.state).sort()).toEqual(['autoTile', 'group', 'groups']);
  });

  it('brings a stored arrangement back', async () => {
    store({ group: 1, groups: [seated('a'), seated('b')], autoTile: false }, 2);
    await usePanes.persist.rehydrate();
    expect(panes().group).toBe(1);
    expect(board()).toEqual(['b']);
    expect(panes().autoTile).toBe(false);
  });

  it('fills in what a stored group is missing', async () => {
    store({ group: 0, groups: [{ layout: 2 }] }, 2);
    await usePanes.persist.rehydrate();
    expect(currentGroup(panes()).boards[3]).toEqual([null, null, null, null]);
    expect(currentSplits(panes())).toEqual({ col: 0.5, rowL: 0.5, rowR: 0.5 });
    expect(panes().autoTile).toBe(true);
  });

  it('falls back to the last group when the stored one is out of range', async () => {
    store({ group: 7, groups: [seated('a'), seated('b')] }, 2);
    await usePanes.persist.rehydrate();
    expect(panes().group).toBe(1);
  });

  it('starts with one blank group when nothing usable is stored', async () => {
    store({ group: 0, groups: 'garbage' }, 2);
    await usePanes.persist.rehydrate();
    expect(panes().groups).toHaveLength(1);
    expect(board()).toEqual([null]);
  });

  it('trims version 1’s fixed four groups down to the last one in use', async () => {
    chatsIn(0, 1);
    store({ group: 0, groups: [seated('a'), emptyGroup, emptyGroup, emptyGroup] }, 1);
    await usePanes.persist.rehydrate();
    // Group II holds a chat in the list though not on its board: it stays.
    expect(panes().groups).toHaveLength(2);
  });

  it('keeps a version 1 group with a seated pane past the last one with chats', async () => {
    chatsIn(0);
    store({ group: 0, groups: [emptyGroup, emptyGroup, seated('a'), emptyGroup] }, 1);
    await usePanes.persist.rehydrate();
    expect(panes().groups).toHaveLength(3);
  });

  it('keeps every version 1 group when the chats cannot be read', async () => {
    localStorage.setItem('luna.chats', '{not json');
    store({ group: 0, groups: [emptyGroup, emptyGroup, emptyGroup, emptyGroup] }, 1);
    await usePanes.persist.rehydrate();
    expect(panes().groups).toHaveLength(4);
  });
});
