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
 * Drift is the page and the window disagreeing *differently from how they did
 * when the page was laid out*. They do not simply agree: Windows' accessibility
 * text scaling reaches WebView2 as a page zoom, so with "Text size" at 150% a
 * page laid out perfectly well reports 1.5× against a window at 1×, and always
 * will. Asserting page == window there reloaded on every launch and then, its
 * one attempt spent, sat out any real drift for the rest of the session. The
 * ratio at load is the one to hold to: a scale change WebView2 handled moves
 * both sides and keeps it; one it dropped moves only the window and breaks it.
 */
export function healScaleDrift() {
  if (!tauriAvailable) return () => {};
  let stopped = false;
  let un: (() => void) | undefined;
  /** page ÷ window as this page was laid out; null until the first reading. */
  let baseline: number | null = null;

  const check = async () => {
    const { getCurrentWindow } = await import('@tauri-apps/api/window');
    const os = await getCurrentWindow().scaleFactor();
    const page = window.devicePixelRatio;
    if (stopped || !os || !page) return;
    const ratio = page / os;
    if (baseline == null) {
      baseline = ratio;
      return;
    }
    if (Math.abs(ratio - baseline) < 0.01) return;

    // A reload that does not fix it must not be tried again on the next tick,
    // or the app spends its life reloading. One attempt per pair of ratios:
    // sessionStorage outlives the reload and dies with the window.
    const key = `luna.dpi-heal:${os}:${page}`;
    if (sessionStorage.getItem(key)) return;
    sessionStorage.setItem(key, '1');
    logWarn(
      'dpi',
      `window is at ${os}× and the page at ${page}×, laid out at ${baseline}:1 — reloading to re-lay it out`
    );
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
