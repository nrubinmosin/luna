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

// Set before the listener is awaited, so a second init (StrictMode mounts
// twice) does not stack a second one.
let listening = false;
// A push that lands while the first answer is out is the newer of the two.
let heard = false;

export const usePower = create<PowerState>(set => ({
  state: IDLE_POWER,
  open: false,

  init: async () => {
    if (!listening) {
      listening = true;
      try {
        await onPowerState(state => {
          heard = true;
          set({ state });
        });
      } catch (e) {
        listening = false;
        throw e;
      }
    }
    const state = await powerState();
    if (!heard) set({ state });
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
