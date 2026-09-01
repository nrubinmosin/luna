import { create } from 'zustand';
import { cliStatus, type CliStatusDto } from '../../ipc/commands';
import { onCliStatus } from '../../ipc/events';

/**
 * The backend's readout for Luna's own copy of the Claude CLI (cli.rs does the
 * actual checking and downloading). One store rather than per-component
 * subscriptions because two places draw it now: the status-bar chip that only
 * appears while something is happening, and the settings dialog's version row.
 */
interface CliState {
  status: CliStatusDto | null;
  /** Subscribes once for the app's lifetime; safe to call again. */
  init: () => void;
}

let started = false;

export const useCli = create<CliState>(set => ({
  status: null,
  init: () => {
    if (started) return;
    started = true;
    void cliStatus().then(s => set({ status: s }));
    // Never unlistened: the store outlives every component that reads it.
    void onCliStatus(s => set({ status: s }));
  }
}));
