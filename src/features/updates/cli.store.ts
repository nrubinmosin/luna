import { create } from 'zustand';
import { cliStatus, cliVersions, type CliStatusDto, type CliVersionsDto } from '../../ipc/cli';
import { onCliCanary, onCliStatus } from '../../ipc/events';
import { PROVIDERS, type Provider } from '../../shared/types';

/**
 * The backend's readout for Luna's own copies of the CLIs (cli.rs does the
 * actual checking and downloading), one per provider: the update pass's
 * status, and the installed versions with what each is to Luna. One store
 * rather than per-component subscriptions because several places draw it:
 * the status-bar chips that only appear while something is happening or a
 * new version's canary has warned, and Settings → CLIs.
 */
interface CliState {
  status: Record<Provider, CliStatusDto | null>;
  versions: Record<Provider, CliVersionsDto | null>;
  /** Subscribes once for the app's lifetime; safe to call again. */
  init: () => void;
  /** Reads a provider's installed versions again, after a change to them. */
  refresh: (p: Provider) => Promise<void>;
}

let started = false;

export const useCli = create<CliState>((set, get) => ({
  status: { claude: null, codex: null },
  versions: { claude: null, codex: null },
  init: () => {
    if (started) return;
    started = true;
    for (const p of PROVIDERS) {
      // A failure is logged by the ipc layer; cli://status fills it in later.
      cliStatus(p)
        .then(s => s && set(st => ({ status: { ...st.status, [p]: s } })))
        .catch(() => {});
      void get().refresh(p);
    }
    // Never unlistened: the store outlives every component that reads it.
    void onCliStatus(s => {
      set(st => ({ status: { ...st.status, [s.provider]: s } }));
      // A pass that has ended may have installed, flipped or swept.
      if (s.phase === 'idle' || s.phase === 'error') void get().refresh(s.provider);
    });
    void onCliCanary(c => void get().refresh(c.provider));
  },
  refresh: async p => {
    const v = await cliVersions(p).catch(() => null);
    if (v) set(st => ({ versions: { ...st.versions, [p]: v } }));
  }
}));

/** The canary of the current version, while it warns and nobody said to keep it. */
/** How many sessions the canary watches (cli/canary.rs `WATCH`). */
export const CANARY_WATCH = 3;

/** Whether `version` got through the canary: its watched sessions all past
 *  their start, none failed. */
export const canaryThrough = (v: CliVersionsDto | null, version: string) =>
  !!v?.canary && v.canary.version === version && v.canary.settled >= CANARY_WATCH && v.canary.failures.length === 0;

export const canaryWarning = (v: CliVersionsDto | null) =>
  v?.canary && v.canary.warned && !v.canary.dismissed ? v.canary : null;
