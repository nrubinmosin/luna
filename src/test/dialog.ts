import { expect } from 'vitest';

/**
 * Asserts the structure that keeps a dialog's action within reach on a short
 * screen (windows.css): `button` is in the .dialog-foot of a .window that sits
 * straight on its .scrim, after that window's .window-body and outside it, so
 * the body is what scrolls and the button stays put. jsdom lays nothing out:
 * this holds the structure the CSS keys on, not where anything lands
 * (src/styles/windows.test.ts holds the rules themselves).
 */
export function expectInDialogFoot(button: HTMLElement) {
  const foot = button.closest('.dialog-foot');
  expect(foot, 'the button is in a .dialog-foot').not.toBeNull();
  expect(button.closest('.window-body'), 'the button is outside the scrolling body').toBeNull();
  const dialog = foot!.parentElement!;
  expect(dialog).toHaveClass('window');
  expect(dialog.parentElement).toHaveClass('scrim');
  const body = dialog.querySelector(':scope > .window-body');
  expect(body, 'the dialog has a .window-body').not.toBeNull();
  expect(
    body!.compareDocumentPosition(foot!) & Node.DOCUMENT_POSITION_FOLLOWING,
    'the foot comes after it'
  ).toBeTruthy();
}
