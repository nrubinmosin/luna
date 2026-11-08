import { create } from 'zustand';
import { coreRestart, coreState, type CoreStateDto } from '../../ipc/coreProcess';
import { onCoreState, onStartOver } from '../../ipc/events';
import { forgetAll } from '../panes/terminals';

/**
 * The window's line to the core, where the sessions run (corelink.rs). When a
 * core that stopped is replaced by a new one, every terminal starts over —
 * its session went with the old core — and resumes from its transcript. The
 * same when the core itself says so: it closed every session for a shutdown of
 * the PC that was then taken back (power/machine.rs).
 */
interface CoreState {
  link: CoreStateDto;
  /** Counts the times the panes have had to start their sessions over. */
  epoch: number;
  init: () => Promise<void>;
  restart: () => void;
  /** Every terminal is made afresh and its session resumed from its transcript. */
  startOver: () => void;
}

/** The one subscription to the link, shared by every `init`: two calls
 *  close together (StrictMode mounts App twice) both found none yet. */
let listening: Promise<unknown> | null = null;
/** The panes have something to start over from: they could not reach a core
 *  (it was down, or another build's). */
let troubled = false;
/** The core the panes' sessions were last started in. */
let lastPid: number | null = null;

const DOWN: CoreStateDto = { state: 'starting', core: null, error: null };

export const useCore = create<CoreState>((set, get) => ({
  link: DOWN,
  epoch: 0,

  init: async () => {
    const take = (link: CoreStateDto) => {
      // Starting again after a core was up: one that let this window go and
      // is gone back to (corelink.rs `rejoin`) has the same pid, and the
      // output it sent meanwhile never came, so the panes start over too.
      if (
        link.state === 'down' ||
        link.state === 'mismatch' ||
        link.state === 'elevated' ||
        (link.state === 'starting' && lastPid != null)
      )
        troubled = true;
      const pid = link.state === 'up' ? (link.core?.pid ?? null) : null;
      const replaced = pid != null && (troubled || (lastPid != null && lastPid !== pid));
      if (pid != null) lastPid = pid;
      set({ link });
      if (replaced) {
        troubled = false;
        get().startOver();
      }
    };
    listening ??= Promise.all([onCoreState(take), onStartOver(() => get().startOver())]);
    await listening;
    take(await coreState());
  },

  // The ipc layer has logged a failure; the banner stays up to say the rest.
  restart: () => void coreRestart().catch(() => {}),

  startOver: () => {
    forgetAll();
    set({ epoch: get().epoch + 1 });
  }
}));
