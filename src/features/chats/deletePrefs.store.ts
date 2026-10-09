import { create } from 'zustand';
import { persist } from 'zustand/middleware';

interface DeletePrefs {
  /**
   * Where the delete dialog's worktree checkbox starts; set in Settings.
   * On by default: a worktree is a throwaway made for its chat, and keeping
   * every one by habit is how a folder fills up with them.
   */
  dropWorktree: boolean;
  setDropWorktree: (on: boolean) => void;
}

export const useDeletePrefs = create<DeletePrefs>()(
  persist(
    set => ({
      dropWorktree: true,
      setDropWorktree: on => set({ dropWorktree: on })
    }),
    {
      name: 'luna.delete',
      partialize: s => ({ dropWorktree: s.dropWorktree })
    }
  )
);
