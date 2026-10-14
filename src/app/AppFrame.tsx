import { useEffect, useState } from 'react';
import type { ReactNode } from 'react';
import iconUrl from '../../src-tauri/icons/32x32.png';
import { BoardTools } from './BoardTools';

const tauriAvailable = '__TAURI_INTERNALS__' in window;

async function appWindow() {
  const { getCurrentWindow } = await import('@tauri-apps/api/window');
  return getCurrentWindow();
}

/**
 * The window's own XP chrome. The native decorations are off, so this strip is
 * the title bar: `data-tauri-drag-region` moves the window (and maximises it on
 * a double-click, which is the platform's own behaviour, not ours). The board's
 * controls sit on it between the name and the window's three buttons, whose
 * glyphs theme.css draws.
 *
 * Closing hides to tray rather than exiting — the Rust side intercepts the
 * close request, so this only has to ask for it.
 */
export function AppFrame({ children }: { children: ReactNode }) {
  // Starts true because the window is configured maximised; a wrong first guess
  // would show a restore glyph for the one frame before the real state lands.
  const [maximized, setMaximized] = useState(true);

  useEffect(() => {
    if (!tauriAvailable) return;
    let disposed = false;
    let un: (() => void) | undefined;
    void (async () => {
      const w = await appWindow();
      const sync = () => void w.isMaximized().then(v => !disposed && setMaximized(v));
      sync();
      const off = await w.onResized(sync);
      if (disposed) off();
      else un = off;
    })();
    return () => {
      disposed = true;
      un?.();
    };
  }, []);

  const act = (fn: (w: Awaited<ReturnType<typeof appWindow>>) => Promise<unknown>) => () => {
    if (!tauriAvailable) return;
    void appWindow().then(fn);
  };

  return (
    <div className="app-frame">
      <div className="title-bar" data-tauri-drag-region>
        <div className="title-bar-text" data-tauri-drag-region>
          {/* XP puts the app's own icon at the head of the title bar. */}
          <img src={iconUrl} alt="" width={16} height={16} />
          Luna
        </div>
        <BoardTools />
        <div className="title-bar-controls">
          <button aria-label="Minimize" onClick={act(w => w.minimize())} />
          <button
            aria-label={maximized ? 'Restore' : 'Maximize'}
            onClick={act(w => w.toggleMaximize())}
          />
          <button aria-label="Close" title="Hide to tray" onClick={act(w => w.close())} />
        </div>
      </div>
      <div className="app-body">{children}</div>
    </div>
  );
}
