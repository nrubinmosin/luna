import { create } from 'zustand';
import { cliStatus, type CliStatusDto } from '../../ipc/commands';
import { onCliStatus } from '../../ipc/events';
import { PROVIDERS, type Provider } from '../../shared/types';

/**
 * The backend's readout for Luna's own copies of the CLIs (cli.rs does the
 * actual checking and downloading), one per provider. One store rather than
 * per-component subscriptions because two places draw it: the status-bar
 * chip that only appears while something is happening, and the settings
 * dialog's version rows.
 */
interface CliState {
  status: Record<Provider, CliStatusDto | null>;
  /** Subscribes once for the app's lifetime; safe to call again. */
  init: () => void;
}

let started = false;

export const useCli = create<CliState>(set => ({
  status: { claude: null, codex: null },
  init: () => {
    if (started) return;
    started = true;
    for (const p of PROVIDERS) {
      void cliStatus(p).then(s => s && set(st => ({ status: { ...st.status, [p]: s } })));
    }
    // Never unlistened: the store outlives every component that reads it.
    void onCliStatus(s => set(st => ({ status: { ...st.status, [s.provider]: s } })));
  }
}));
