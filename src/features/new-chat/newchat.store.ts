import { create } from 'zustand';

interface NewChatUi {
  open: boolean;
  initialFolder: string | null;
  openDialog: (folder?: string) => void;
  close: () => void;
}

export const useNewChat = create<NewChatUi>()(set => ({
  open: false,
  initialFolder: null,
  openDialog: folder => set({ open: true, initialFolder: folder ?? null }),
  close: () => set({ open: false, initialFolder: null })
}));
