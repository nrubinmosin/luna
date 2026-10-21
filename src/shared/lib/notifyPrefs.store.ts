import { create } from 'zustand';
import { persist } from 'zustand/middleware';

interface NotifyPrefs {
  /** Whether a desktop notification plays the system's sound; set in Settings. */
  sound: boolean;
  setSound: (on: boolean) => void;
}

export const useNotifyPrefs = create<NotifyPrefs>()(
  persist(
    set => ({
      sound: true,
      setSound: on => set({ sound: on })
    }),
    {
      name: 'luna.notify',
      partialize: s => ({ sound: s.sound })
    }
  )
);
