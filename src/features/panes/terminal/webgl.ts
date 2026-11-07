import type { Terminal as XTerm } from '@xterm/xterm';
import { WebglAddon } from '@xterm/addon-webgl';
import { logWarn } from '../../../shared/lib/log';
import { safely } from './safely';

/**
 * Puts the terminal on the webgl renderer where there is one, and returns the
 * call that takes it off again. `onLoss` runs once the renderer has been
 * dropped for a lost context, the terminal back on the DOM renderer.
 */
export function loadWebgl(term: XTerm, onLoss: () => void): () => void {
  let webgl: WebglAddon | null = null;
  let ctxLossSub: { dispose(): void } | null = null;
  // Dispose the addon through its own dispose(): loadAddon replaced that
  // method with the AddonManager's wrapper, which is idempotent and marks the
  // addon disposed so term.dispose() skips it later. The previous fix here
  // bound the wrapper, then swapped in a no-op — which worked, but only by
  // accident of the wrapper's own bookkeeping, and it hid the real story:
  // the addon's genuine first dispose throws a TypeError from deep inside
  // its listener teardown (upstream @xterm/addon-webgl, after the renderer
  // and its GL context are already released). That known-benign throw was
  // being logged as a teardown warning on every single pane close — 2k+
  // lines a day. Drop our own context-loss subscription first (it is the
  // teardown path the throw walks through), and if the addon still throws a
  // bare TypeError, swallow it; anything else is a real failure and is kept.
  const disposeWebgl = () => {
    const addon = webgl;
    webgl = null;
    if (!addon) return;
    safely(() => ctxLossSub?.dispose());
    ctxLossSub = null;
    try {
      addon.dispose();
    } catch (e) {
      if (!(e instanceof TypeError)) {
        logWarn('terminal', `webgl dispose failed: ${(e as Error)?.stack ?? String(e)}`);
      }
    }
  };
  try {
    webgl = new WebglAddon();
    ctxLossSub = webgl.onContextLoss(() => {
      // Chromium force-loses the oldest context once too many are live, and
      // the pane that owns it is usually still on screen. Falling back to the
      // DOM renderer silently would leave it blank until the next keystroke.
      disposeWebgl();
      onLoss();
    });
    term.loadAddon(webgl);
  } catch (e) {
    // Slower, and otherwise invisible: the terminal still draws.
    logWarn('terminal', `webgl renderer unavailable, using the DOM renderer: ${(e as Error)?.message ?? String(e)}`);
    webgl = null;
  }
  return disposeWebgl;
}
