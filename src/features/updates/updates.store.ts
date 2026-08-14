import { create } from 'zustand';
import type { Update } from '@tauri-apps/plugin-updater';
import { logInfo, logWarn } from '../../shared/lib/log';

const tauriAvailable = '__TAURI_INTERNALS__' in window;

/**
 * Where the app is in the update cycle. 'idle' covers both "nothing on offer"
 * and "haven't looked yet"; `checkedAt` is what separates them for the label.
 */
export type Phase = 'idle' | 'checking' | 'available' | 'downloading' | 'error';

// The handle the plugin hands back is a live object with a download method on
// it, not data — it stays out of the store, which is for what the UI draws.
let pending: Update | null = null;

interface UpdatesState {
  phase: Phase;
  /** The running build, for the label. */
  current: string;
  /** The release on offer, once there is one. */
  next: string | null;
  notes: string | null;
  /** Bytes in, and the total — only when the server declared a length. */
  got: number;
  total: number | null;
  error: string | null;
  /** When a check last came back clean, so the field can say so. */
  checkedAt: number | null;
  /** True while the dialog with the release notes is up. */
  asking: boolean;
  /** `manual` for a check the user asked for: those report their failures,
   *  the one on startup stays quiet about being offline. */
  check: (manual: boolean) => Promise<void>;
  /** Downloads, installs, and — on Windows — never returns: see below. */
  install: () => Promise<void>;
  setAsking: (v: boolean) => void;
}

export const useUpdates = create<UpdatesState>(set => ({
  phase: 'idle',
  current: '',
  next: null,
  notes: null,
  got: 0,
  total: null,
  error: null,
  checkedAt: null,
  asking: false,

  check: async (manual: boolean) => {
    if (!tauriAvailable) return;
    // A second check while one is in flight, or on top of a finished download,
    // would drop the handle the restart button is waiting on.
    const { phase } = useUpdates.getState();
    if (phase === 'checking' || phase === 'downloading') return;

    set({ phase: 'checking', error: null });
    try {
      const [{ check }, { getVersion }] = await Promise.all([
        import('@tauri-apps/plugin-updater'),
        import('@tauri-apps/api/app')
      ]);
      set({ current: await getVersion() });
      const update = await check();
      if (!update) {
        pending = null;
        set({ phase: 'idle', next: null, notes: null, checkedAt: Date.now() });
        return;
      }
      pending = update;
      logInfo('updater', `${update.version} is available`);
      set({
        phase: 'available',
        next: update.version,
        notes: update.body?.trim() || null,
        checkedAt: Date.now()
      });
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      logWarn('updater', `check failed: ${msg}`);
      // Offline, or no release published yet. Only a check that was asked for
      // gets to put a warning in the status bar over it.
      set(manual ? { phase: 'error', error: msg } : { phase: 'idle' });
    }
  },

  install: async () => {
    if (!pending) return;
    set({ phase: 'downloading', asking: false, got: 0, total: null, error: null });
    try {
      await pending.downloadAndInstall(ev => {
        if (ev.event === 'Started') set({ total: ev.data.contentLength ?? null });
        else if (ev.event === 'Progress') set(s => ({ got: s.got + ev.data.chunkLength }));
      });
      // Unreachable on Windows: the plugin hands the installer to the shell and
      // ends the process itself with exit(0), because NSIS cannot replace the
      // exe of a running app. It passes /UPDATE and this process's own
      // arguments, which is what has the installer start Luna again afterwards.
      // So there is no "installed, restart when you like" state to offer, and
      // nothing after this line runs.
      logInfo('updater', `${pending.version} installed`);
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      logWarn('updater', `install failed: ${msg}`);
      set({ phase: 'error', error: msg });
    }
  },

  setAsking: (v: boolean) => set({ asking: v })
}));
