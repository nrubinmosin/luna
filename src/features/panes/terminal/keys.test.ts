import type { Terminal as XTerm } from '@xterm/xterm';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { attachKeys } from './keys';

/** The key handler as xterm would be handed it, over a selection the test sets. */
function setup(selection: string) {
  let handler: (e: KeyboardEvent) => boolean = () => true;
  const term = {
    attachCustomKeyEventHandler: (h: typeof handler) => (handler = h),
    hasSelection: () => selection !== '',
    getSelection: () => selection,
    clearSelection: vi.fn(),
    input: vi.fn()
  };
  attachKeys(term as unknown as XTerm);
  const press = (code: string, mods: Partial<KeyboardEventInit> = { ctrlKey: true }, type = 'keydown') => {
    const e = new KeyboardEvent(type, { code, key: code, cancelable: true, ...mods });
    return { passed: handler(e), prevented: e.defaultPrevented };
  };
  return { term, press };
}

const writeText = vi.fn(() => Promise.resolve());

beforeEach(() => {
  writeText.mockClear();
  Object.defineProperty(navigator, 'clipboard', { value: { writeText }, configurable: true });
});

describe('terminal keys: Ctrl+C and Ctrl+V', () => {
  it('copies a selection on Ctrl+C instead of sending ^C, and clears it', () => {
    const { term, press } = setup('some output');
    expect(press('KeyC')).toEqual({ passed: false, prevented: true });
    expect(writeText).toHaveBeenCalledWith('some output');
    expect(term.clearSelection).toHaveBeenCalledOnce();
  });

  it('sends ^C as ever with nothing selected', () => {
    const { press } = setup('');
    expect(press('KeyC').passed).toBe(true);
    expect(writeText).not.toHaveBeenCalled();
  });

  it('leaves Ctrl+V to the browser, so a paste reaches the pane', () => {
    const { press } = setup('');
    expect(press('KeyV')).toEqual({ passed: false, prevented: false });
  });

  it('lets Ctrl+Shift+C and other modified keys through to the terminal', () => {
    const { press } = setup('some output');
    expect(press('KeyC', { ctrlKey: true, shiftKey: true }).passed).toBe(true);
    expect(press('KeyC', {}).passed).toBe(true);
    expect(writeText).not.toHaveBeenCalled();
  });
});

describe('terminal keys: Alt+arrow', () => {
  const platform = (name: string) => Object.defineProperty(navigator, 'platform', { value: name, configurable: true });
  beforeEach(() => platform('Win32'));

  it('sends Ctrl+arrow for Alt+arrow, as xterm did before 6', () => {
    const { term, press } = setup('');
    expect(press('ArrowLeft', { altKey: true })).toEqual({ passed: false, prevented: true });
    expect(press('ArrowRight', { altKey: true }).passed).toBe(false);
    expect(press('ArrowUp', { altKey: true }).passed).toBe(false);
    expect(press('ArrowDown', { altKey: true }).passed).toBe(false);
    expect(term.input.mock.calls).toEqual([['[1;5D'], ['[1;5C'], ['[1;5A'], ['[1;5B']]);
  });

  it('sends a word move on macOS, and leaves Alt+Up and Alt+Down to xterm there', () => {
    platform('MacIntel');
    const { term, press } = setup('');
    expect(press('ArrowLeft', { altKey: true }).passed).toBe(false);
    expect(press('ArrowRight', { altKey: true }).passed).toBe(false);
    expect(press('ArrowUp', { altKey: true }).passed).toBe(true);
    expect(term.input.mock.calls).toEqual([['b'], ['f']]);
  });

  it('leaves other modifiers, a key up and a bare arrow to xterm', () => {
    const { term, press } = setup('');
    expect(press('ArrowLeft', { altKey: true, shiftKey: true }).passed).toBe(true);
    expect(press('ArrowLeft', { altKey: true, ctrlKey: true }).passed).toBe(true);
    expect(press('ArrowLeft', { altKey: true }, 'keyup').passed).toBe(true);
    expect(press('ArrowLeft', {}).passed).toBe(true);
    expect(term.input).not.toHaveBeenCalled();
  });
});
