import { create } from 'zustand';

/** Whether Settings is open. `?demo=settings` opens it for its screenshot;
 *  the check compiles away in a release build. */
export const useSettings = create<{ open: boolean; setOpen: (v: boolean) => void }>(set => ({
  open: import.meta.env.DEV && new URLSearchParams(location.search).get('demo') === 'settings',
  setOpen: v => set({ open: v })
}));
