import { create } from 'zustand';
import { coreRestart, coreState, type CoreStateDto } from '../../ipc/commands';
import { onCoreState } from '../../ipc/events';
import { forgetAll } from '../panes/terminals';

/**
 * The window's line to the core, where the sessions run (corelink.rs). When a
 * core that stopped is replaced by a new one, every terminal starts over —
 * its session went with the old core — and resumes from its transcript.
 */
interface CoreState {
  link: CoreStateDto;
  /** Counts the cores the panes have had to start over for. */
  epoch: number;
  init: () => Promise<void>;
  restart: () => void;
}

let unlisten: (() => void) | null = null;
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
      if (link.state === 'down' || link.state === 'mismatch') troubled = true;
      const pid = link.state === 'up' ? (link.core?.pid ?? null) : null;
      const replaced = pid != null && (troubled || (lastPid != null && lastPid !== pid));
      if (pid != null) lastPid = pid;
      if (replaced) {
        troubled = false;
        forgetAll();
        set({ link, epoch: get().epoch + 1 });
      } else {
        set({ link });
      }
    };
    if (!unlisten) unlisten = await onCoreState(take);
    take(await coreState());
  },

  restart: () => void coreRestart()
}));
