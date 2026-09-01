import { logWarn } from '../shared/lib/log';

const tauriAvailable = '__TAURI_INTERNALS__' in window;

/**
 * Windows lets a monitor's scale change under a running window, and WebView2
 * does not always carry that into the page it is already showing: the renderer
 * keeps laying out for the old ratio while the window and its input are on the
 * new one. Everything then sits a fraction of the screen away from where it is
 * drawn — hover lights up the wrong control, a click lands beside the button,
 * and the far edges of the layout run off the window. The window itself is
 * fine, which is what makes it so confusing to look at: at the Win32 level the
 * client area, the DPI and the webview's own child windows all agree.
 *
 * Nothing inside the page can re-do that layout — but loading it again does,
 * at whatever ratio is current. Here that costs almost nothing: the sessions
 * live in the Rust side, so every pane comes straight back with its scrollback.
 *
 * Only on a real mismatch, so a scale change WebView2 did handle (moving the
 * window to a monitor with a different DPI, usually) costs nothing at all.
 */
export function healScaleDrift() {
  if (!tauriAvailable) return () => {};
  let stopped = false;
  let un: (() => void) | undefined;

  const check = async () => {
    const { getCurrentWindow } = await import('@tauri-apps/api/window');
    const os = await getCurrentWindow().scaleFactor();
    const page = window.devicePixelRatio;
    if (stopped || Math.abs(os - page) < 0.01) return;

    // A reload that does not fix it must not be tried again on the next tick,
    // or the app spends its life reloading. One attempt per pair of ratios:
    // sessionStorage outlives the reload and dies with the window.
    const key = `luna.dpi-heal:${os}:${page}`;
    if (sessionStorage.getItem(key)) return;
    sessionStorage.setItem(key, '1');
    logWarn('dpi', `window is at ${os}× and the page at ${page}× — reloading to re-lay it out`);
    location.reload();
  };

  void (async () => {
    const { getCurrentWindow } = await import('@tauri-apps/api/window');
    // The event arrives with the change, not after the webview has answered
    // it; give it a beat before asking who agrees with whom.
    const off = await getCurrentWindow().onScaleChanged(() => setTimeout(() => void check(), 400));
    if (stopped) off();
    else un = off;
  })();

  // A drift that happened while the app was in the tray, or one WebView2 never
  // reported, would otherwise sit there until the next restart.
  const timer = setInterval(() => void check(), 60_000);
  void check();

  return () => {
    stopped = true;
    clearInterval(timer);
    un?.();
  };
}
