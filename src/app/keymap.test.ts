import { fireEvent, renderHook } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { ClaudeChat } from '../shared/types';
import { useChats } from '../features/chats/chats.store';
import { quickChat } from '../features/new-chat/create';
import { currentLayout, currentSlots, usePanes } from '../features/panes/panes.store';
import { useShortcuts } from '../features/shortcuts/ShortcutsDialog';
import { useKeymap } from './keymap';
import { useShell } from './shell.store';

// xterm cannot run in jsdom; the keymap only asks a terminal to take focus.
vi.mock('../features/panes/terminals', () => ({
  focusTerminal: vi.fn(),
  forget: vi.fn(),
  forgetAll: vi.fn(),
  startSession: vi.fn()
}));
vi.mock('../features/new-chat/create', () => ({ quickChat: vi.fn(async () => {}), createChat: vi.fn() }));

const panes = () => usePanes.getState();

/** Presses a key the way a focused terminal would see it, and says whether Luna took it. */
const press = (init: KeyboardEventInit) => !fireEvent.keyDown(document.body, init);
const ctrl = (key: string, code: string, more: KeyboardEventInit = {}) => press({ key, code, ctrlKey: true, ...more });
const digit = (n: number) => ({ key: String(n), code: `Digit${n}` });

const chat = (id: string, over: Partial<ClaudeChat> = {}): ClaudeChat => ({
  id,
  name: id,
  status: 'resting',
  context: 0,
  account: 'work',
  group: 0,
  worktree: false,
  provider: 'claude',
  model: 'Opus',
  effort: 'high',
  perm: 'Bypass',
  ...over
});

function mount() {
  const openNewChat = vi.fn();
  const closeModal = vi.fn();
  const view = renderHook(() => useKeymap({ openNewChat, closeModal }));
  return { openNewChat, closeModal, unmount: view.unmount };
}

function board(slots: Array<string | null>) {
  usePanes.setState({ autoTile: false });
  panes().setLayout(slots.length as 1 | 2 | 3 | 4);
  slots.forEach((id, i) => id && panes().dropChat(i, id));
}

beforeEach(() => vi.clearAllMocks());

describe('keymap: new chats', () => {
  it('opens the new-chat dialog on Ctrl+N and keeps the key from the terminal', () => {
    const { openNewChat } = mount();
    expect(ctrl('n', 'KeyN')).toBe(true);
    expect(openNewChat).toHaveBeenCalledOnce();
  });

  it('takes Ctrl+N by its physical key on a Cyrillic layout', () => {
    const { openNewChat } = mount();
    ctrl('т', 'KeyN');
    expect(openNewChat).toHaveBeenCalledOnce();
  });

  it('starts a quick chat with no dialog on Ctrl+Shift+N', () => {
    const { openNewChat } = mount();
    ctrl('N', 'KeyN', { shiftKey: true });
    expect(quickChat).toHaveBeenCalledOnce();
    expect(openNewChat).not.toHaveBeenCalled();
  });

  it('accepts Cmd as well as Ctrl', () => {
    const { openNewChat } = mount();
    press({ key: 'n', code: 'KeyN', metaKey: true });
    expect(openNewChat).toHaveBeenCalledOnce();
  });
});

describe('keymap: layouts and panes', () => {
  it('switches the layout on Ctrl+Shift+1…4', () => {
    mount();
    for (const n of [4, 2, 3, 1] as const) {
      ctrl('!', `Digit${n}`, { shiftKey: true });
      expect(currentLayout(panes())).toBe(n);
    }
  });

  it('moves into pane N on Ctrl+N with several panes, putting the sheet down', () => {
    board(['a', 'b', 'c']);
    panes().setPeek(0);
    mount();
    expect(ctrl('3', 'Digit3')).toBe(true);
    expect(panes().activePane).toBe(2);
    expect(panes().peek).toBeNull();
  });

  it('lets a digit past the last pane through to the terminal', () => {
    board(['a', 'b']);
    panes().setActivePane(0);
    mount();
    expect(ctrl('3', 'Digit3')).toBe(false);
    expect(panes().activePane).toBe(0);
  });

  it('reads the numeric keypad the same as the digit row', () => {
    board(['a', 'b']);
    panes().setActivePane(0);
    mount();
    ctrl('2', 'Numpad2');
    expect(panes().activePane).toBe(1);
  });

  it('puts the sheet down on Ctrl+0', () => {
    board(['a', 'b']);
    panes().setPeekChat('x');
    mount();
    expect(ctrl('0', 'Digit0')).toBe(true);
    expect(panes().peekChat).toBeNull();
  });

  it('with one pane, opens the Nth chat of the list on Ctrl+N', () => {
    useChats.getState().addChat('/repo', chat('a'), true);
    useChats.getState().addChat('/repo', chat('b'), true);
    mount();
    ctrl('2', 'Digit2');
    expect(currentSlots(panes())[0]).toBe('b');
    expect(useChats.getState().active).toBe('b');
  });

  it('with one pane, lets a digit past the end of the list through', () => {
    useChats.getState().addChat('/repo', chat('a'), true);
    mount();
    expect(ctrl('5', 'Digit5')).toBe(false);
  });

  it('counts only the chats of the group on screen', () => {
    useChats.getState().addChat('/repo', chat('a', { group: 1 }), true);
    useChats.getState().addChat('/repo', chat('b'), true);
    mount();
    ctrl('1', 'Digit1');
    expect(currentSlots(panes())[0]).toBe('b');
  });
});

describe('keymap: Alt+digit holds a pane up', () => {
  const alt = (n: number) => press({ ...digit(n), altKey: true });

  it('holds the pane up on the sheet, and the same keys put it back', () => {
    board(['a', 'b']);
    mount();
    expect(alt(2)).toBe(true);
    expect(panes().peek).toBe(1);
    expect(panes().activePane).toBe(1);
    alt(2);
    expect(panes().peek).toBeNull();
  });

  it('does nothing with a single pane', () => {
    board(['a']);
    mount();
    expect(alt(1)).toBe(false);
    expect(panes().peek).toBeNull();
  });

  it('leaves AltGr (Ctrl+Alt) alone, which types characters on some layouts', () => {
    board(['a', 'b']);
    mount();
    expect(press({ ...digit(2), altKey: true, ctrlKey: true })).toBe(false);
    expect(panes().peek).toBeNull();
  });
});

describe('keymap: window chords', () => {
  it('pins and unpins the sidebar on Ctrl+Shift+B', () => {
    mount();
    ctrl('B', 'KeyB', { shiftKey: true });
    expect(useShell.getState().pinned).toBe(false);
    ctrl('B', 'KeyB', { shiftKey: true });
    expect(useShell.getState().pinned).toBe(true);
  });

  it('lets plain Ctrl+B through to the CLI', () => {
    mount();
    expect(ctrl('b', 'KeyB')).toBe(false);
    expect(useShell.getState().pinned).toBe(true);
  });

  it('opens a new group on Ctrl+Shift+T', () => {
    mount();
    ctrl('T', 'KeyT', { shiftKey: true });
    expect(panes().groups).toHaveLength(2);
    expect(panes().group).toBe(1);
  });

  it('goes to the next chat waiting for you on Ctrl+Shift+W, in its own group', () => {
    useChats.getState().addChat('/repo', chat('a'), true);
    useChats.getState().addChat('/repo', chat('w', { group: 1, status: 'waiting' }), true);
    panes().coverGroup(1);
    mount();
    expect(ctrl('W', 'KeyW', { shiftKey: true })).toBe(true);
    expect(panes().group).toBe(1);
    expect(useChats.getState().active).toBe('w');
  });

  it('walks the groups round on Ctrl+Tab', () => {
    panes().addGroup();
    mount();
    ctrl('Tab', 'Tab');
    expect(panes().group).toBe(0);
  });

  it('leaves Ctrl+Shift+Tab to Claude Code', () => {
    panes().addGroup();
    mount();
    expect(ctrl('Tab', 'Tab', { shiftKey: true })).toBe(false);
    expect(panes().group).toBe(1);
  });

  it('lets an ordinary Ctrl+key through, e.g. Ctrl+C', () => {
    mount();
    expect(ctrl('c', 'KeyC')).toBe(false);
  });

  it('toggles the shortcuts sheet on F1', () => {
    mount();
    press({ key: 'F1', code: 'F1' });
    expect(useShortcuts.getState().open).toBe(true);
    press({ key: 'F1', code: 'F1' });
    expect(useShortcuts.getState().open).toBe(false);
  });
});

describe('keymap: Escape', () => {
  it('closes the open modal and still reaches the terminal', () => {
    const { closeModal } = mount();
    expect(press({ key: 'Escape', code: 'Escape' })).toBe(false);
    expect(closeModal).toHaveBeenCalledOnce();
  });

  it('leaves an open account list to close itself first', () => {
    const { closeModal } = mount();
    const picker = document.createElement('div');
    picker.setAttribute('data-picker-open', 'true');
    document.body.append(picker);
    try {
      press({ key: 'Escape', code: 'Escape' });
      expect(closeModal).not.toHaveBeenCalled();
    } finally {
      picker.remove();
    }
  });
});

it('stops listening once unmounted', () => {
  const { openNewChat, unmount } = mount();
  unmount();
  expect(ctrl('n', 'KeyN')).toBe(false);
  expect(openNewChat).not.toHaveBeenCalled();
});
