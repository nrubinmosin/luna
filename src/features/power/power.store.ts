import { create } from 'zustand';
import {
  armPowerOff, disarmPowerOff, IDLE_POWER, powerState, setKeepAwake,
  type PowerAction, type PowerStateDto
} from '../../ipc/commands';
import { onPowerState } from '../../ipc/events';
import { logWarn } from '../../shared/lib/log';

/**
 * The machine's side of the sessions, as Rust tracks it (power.rs): whether
 * the awake hold is on, whether a power-off is armed, and what is keeping it
 * from firing. Rust pushes every change as `power://state`; the store asks
 * once at start and otherwise only listens.
 */
interface PowerState {
  state: PowerStateDto;
  /** True while the menu is up. */
  open: boolean;
  init: () => Promise<void>;
  setOpen: (v: boolean) => void;
  keepAwake: (on: boolean) => Promise<void>;
  arm: (action: PowerAction, quietS: number) => Promise<void>;
  disarm: () => Promise<void>;
}

let unlisten: (() => void) | null = null;

export const usePower = create<PowerState>(set => ({
  state: IDLE_POWER,
  open: false,

  init: async () => {
    if (!unlisten) unlisten = await onPowerState(state => set({ state }));
    set({ state: await powerState() });
  },

  setOpen: v => set({ open: v }),

  keepAwake: async on => {
    try {
      set({ state: await setKeepAwake(on) });
    } catch (e) {
      logWarn('power', `keep awake: ${String(e)}`);
    }
  },

  arm: async (action, quietS) => {
    try {
      set({ state: await armPowerOff(action, quietS), open: false });
    } catch (e) {
      logWarn('power', `arm: ${String(e)}`);
    }
  },

  disarm: async () => {
    try {
      set({ state: await disarmPowerOff() });
    } catch (e) {
      logWarn('power', `disarm: ${String(e)}`);
    }
  }
}));
