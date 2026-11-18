import { create } from 'zustand';

/**
 * The worktrees a chat's delete was asked to take and kept, another session
 * being in them or on its way in: said on the status bar until clicked away.
 */
interface KeptWorktrees {
  paths: string[];
  keep: (path: string) => void;
  dismiss: () => void;
}

export const useKeptWorktrees = create<KeptWorktrees>(set => ({
  paths: [],
  keep: path => set(s => ({ paths: s.paths.includes(path) ? s.paths : [...s.paths, path] })),
  dismiss: () => set({ paths: [] })
}));
