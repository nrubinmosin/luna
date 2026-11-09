import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import windowsCss from './windows.css?raw';
import controlsCss from './controls.css?raw';

// The sheets as the app loads them. jsdom cascades their rules into
// getComputedStyle and works out a calc() against its window, but lays
// nothing out: this holds the rules and what they match, not where anything
// lands (the dialogs' tests hold their structure, src/test/dialog.ts).
let sheet: HTMLStyleElement;
beforeEach(() => {
  sheet = document.createElement('style');
  sheet.textContent = controlsCss + windowsCss;
  document.head.append(sheet);
});
afterEach(() => {
  sheet.remove();
  document.body.replaceChildren();
});

/** A dialog the way every one is built, a window that is not one, and a popover. */
function mount() {
  document.body.innerHTML = `
    <div data-app>
      <div class="scrim">
        <div class="window" id="dialog">
          <div class="title-bar"></div>
          <div class="window-body"></div>
          <div class="dialog-foot"></div>
        </div>
      </div>
      <div class="window" id="plain"><div class="window-body"></div></div>
      <div class="popover"></div>
    </div>`;
  return (selector: string) => getComputedStyle(document.querySelector(selector)!);
}

describe('a dialog on a short screen', () => {
  it('is held to the window less a margin, a column with its title bar and foot fixed', () => {
    const $ = mount();
    expect($('#dialog').display).toBe('flex');
    expect($('#dialog').flexDirection).toBe('column');
    expect($('#dialog').maxHeight).toBe(`${window.innerHeight - 48}px`);
    expect($('#dialog').maxWidth).toBe(`${window.innerWidth - 32}px`);
    expect($('#dialog > .title-bar').flexShrink).toBe('0');
    expect($('#dialog > .dialog-foot').flexShrink).toBe('0');
  });

  it('scrolls its body, which may shrink below its content', () => {
    const $ = mount();
    expect($('#dialog > .window-body').overflowY).toBe('auto');
    expect($('#dialog > .window-body').minHeight).toBe('0px');
    expect($('#dialog > .window-body').flexShrink).toBe('1');
  });

  it('leaves a window that is no dialog as it was', () => {
    const $ = mount();
    expect($('#plain').maxHeight).toBe('none');
    expect($('#plain > .window-body').overflowY).toBe('visible');
  });

  it('caps a popover at the window and scrolls it', () => {
    const $ = mount();
    expect($('.popover').maxHeight).toBe(`${window.innerHeight - 16}px`);
    expect($('.popover').overflowY).toBe('auto');
  });
});
