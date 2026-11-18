import type { Terminal as XTerm } from '@xterm/xterm';
import { WebglAddon } from '@xterm/addon-webgl';
import { logWarn } from '../../../shared/lib/log';
import { safely } from './safely';

/**
 * Puts the terminal on the webgl renderer where there is one, and returns the
 * call that takes it off again, to make before the terminal is disposed.
 *
 * Taking the addon off is what puts the terminal back on its DOM renderer: the
 * addon's own teardown hands the terminal a new one. So a context the browser
 * takes away for good (Chromium force-loses the oldest once too many are live,
 * and the pane that owns it is usually still on screen) disposes the addon,
 * and `onLoss` runs after, with the DOM renderer in place, to have the pane
 * repainted.
 */
export function loadWebgl(term: XTerm, onLoss: () => void = () => {}): () => void {
  let webgl: WebglAddon | null = null;
  // Through the addon's dispose, which loadAddon replaced with the terminal's
  // own wrapper: it runs the teardown once, and term.dispose() skips an addon
  // already disposed. The addon's emitters, its context-loss one among them,
  // go with it.
  const disposeWebgl = () => {
    const addon = webgl;
    webgl = null;
    if (!addon) return;
    try {
      addon.dispose();
    } catch (e) {
      // A throw escaping a React effect cleanup takes the root down, so a
      // failure here is logged and goes no further.
      logWarn('terminal', `webgl dispose failed: ${(e as Error)?.stack ?? String(e)}`);
    }
  };
  let addon: WebglAddon | null = null;
  try {
    addon = new WebglAddon();
    addon.onContextLoss(() => {
      disposeWebgl();
      onLoss();
    });
    term.loadAddon(addon);
    webgl = addon;
  } catch (e) {
    // Slower, and otherwise invisible: the terminal still draws.
    logWarn('terminal', `webgl renderer unavailable, using the DOM renderer: ${(e as Error)?.message ?? String(e)}`);
    // Loaded but never activated, it still sits in the terminal's addons.
    const failed = addon;
    if (failed) safely(() => failed.dispose());
  }
  return disposeWebgl;
}
