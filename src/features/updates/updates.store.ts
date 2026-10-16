import { create } from 'zustand';
import type { Update } from '@tauri-apps/plugin-updater';
import { noteExit, stopAllSessions } from '../../ipc/commands';
import { useCore } from '../core/core.store';
import { logInfo, logWarn } from '../../shared/lib/log';

const tauriAvailable = '__TAURI_INTERNALS__' in window;

/**
 * Where the app is in the update cycle. 'idle' covers both "nothing on offer"
 * and "haven't looked yet"; `checkedAt` is what separates them for the label.
 */
export type Phase = 'idle' | 'checking' | 'available' | 'downloading' | 'stopping' | 'error';

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
  /** The core fingerprint the release on offer was built with (release.ps1),
   *  if it says. */
  nextCore: string | null;
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

/** Whether installing a release built with this core fingerprint leaves the
 *  running core, and the sessions in it, alone. */
export const keepsSessions = (nextCore: string | null): boolean => {
  const link = useCore.getState().link;
  return !!nextCore && link.state === 'up' && link.core?.fingerprint === nextCore;
};

export const useUpdates = create<UpdatesState>(set => ({
  phase: 'idle',
  current: '',
  next: null,
  notes: null,
  nextCore: null,
  got: 0,
  total: null,
  error: null,
  checkedAt: null,
  asking: false,

  check: async (manual: boolean) => {
    if (!tauriAvailable) return;
    // A second check while one is in flight, or on top of a download, would
    // drop the update handle the download is running on.
    const { phase } = useUpdates.getState();
    if (phase === 'checking' || phase === 'downloading' || phase === 'stopping') return;

    set({ phase: 'checking', error: null });
    try {
      const [{ check }, { getVersion }] = await Promise.all([
        import('@tauri-apps/plugin-updater'),
        import('@tauri-apps/api/app')
      ]);
      set({ current: await getVersion() });
      // The test build (test-build.ps1) must never install a release over itself.
      const update = import.meta.env.VITE_LUNA_FLAVOR === 'test' ? null : await check();
      if (!update) {
        pending = null;
        set({ phase: 'idle', next: null, notes: null, nextCore: null, checkedAt: Date.now() });
        return;
      }
      pending = update;
      logInfo('updater', `${update.version} is available`);
      set({
        phase: 'available',
        next: update.version,
        notes: update.body?.trim() || null,
        nextCore: typeof update.rawJson.core_fingerprint === 'string' ? update.rawJson.core_fingerprint : null,
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
      await pending.download(ev => {
        if (ev.event === 'Started') set({ total: ev.data.contentLength ?? null });
        else if (ev.event === 'Progress') set(s => ({ got: s.got + ev.data.chunkLength }));
      });
      // An update that leaves the core as it is restarts only the window: the
      // core and every session in it carry on, and the new window attaches.
      // Otherwise — only once the installer is on disk, so a download that
      // fails costs nothing — every CLI is asked to quit, each with its exit
      // hooks, rather than going down with the old core mid-write.
      const keep = keepsSessions(useUpdates.getState().nextCore);
      if (!keep) {
        set({ phase: 'stopping' });
        await stopAllSessions();
      }
      // Said before, because the install never returns on Windows; taken back
      // below if it fails.
      await noteExit(`installing the update to ${pending.version}${keep ? '; the sessions keep running in the core' : ''}`);
      await pending.install();
      // Unreachable on Windows: the plugin hands the installer to the shell and
      // ends the process itself with exit(0), because NSIS cannot replace the
      // exe of a running app. It passes /UPDATE and this process's own
      // arguments, which is what has the installer start Luna again afterwards.
      // So there is no "installed, restart when you like" state to offer, and
      // nothing after this line runs.
      logInfo('updater', `${pending.version} installed`);
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      await noteExit(null).catch(() => {});
      logWarn('updater', `install failed: ${msg}`);
      set({ phase: 'error', error: msg });
    }
  },

  setAsking: (v: boolean) => set({ asking: v })
}));
