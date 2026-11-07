import type { Terminal as XTerm } from '@xterm/xterm';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { attachClipboardKeys } from './keys';

/** The key handler as xterm would be handed it, over a selection the test sets. */
function setup(selection: string) {
  let handler: (e: KeyboardEvent) => boolean = () => true;
  const term = {
    attachCustomKeyEventHandler: (h: typeof handler) => (handler = h),
    hasSelection: () => selection !== '',
    getSelection: () => selection,
    clearSelection: vi.fn()
  };
  attachClipboardKeys(term as unknown as XTerm);
  const press = (code: string, mods: Partial<KeyboardEventInit> = { ctrlKey: true }) => {
    const e = new KeyboardEvent('keydown', { code, cancelable: true, ...mods });
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
