import { create } from 'zustand';

/** The Settings dialog's pages, in the order its navigation lists them. */
export const SETTINGS_PAGES = ['general', 'chats', 'agents', 'links', 'clis', 'updates'] as const;
export type SettingsPage = (typeof SETTINGS_PAGES)[number];

/** Whether Settings is open, and on which page. The page outlives a close, so
 *  the dialog comes back where it was left; a caller after one setting names
 *  its page. */
export const useSettings = create<{
  open: boolean;
  page: SettingsPage;
  setOpen: (v: boolean, page?: SettingsPage) => void;
  setPage: (page: SettingsPage) => void;
}>(set => ({
  open: false,
  page: 'general',
  setOpen: (v, page) => set(page ? { open: v, page } : { open: v }),
  setPage: page => set({ page })
}));
