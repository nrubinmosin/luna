import { create } from 'zustand';
import { persist } from 'zustand/middleware';

export type Layout = 1 | 2 | 3 | 4;

interface PanesState {
  layout: Layout;
  panes: (string | null)[];
  over: number;
  drag: string | null;
  setLayout: (n: Layout) => void;
  dropChat: (paneIndex: number, chatId: string) => void;
  closePane: (paneIndex: number) => void;
  evictChat: (chatId: string) => void;
  autoPlace: (chatId: string) => void;
  setOver: (i: number) => void;
  setDrag: (id: string | null) => void;
}

export const usePanes = create<PanesState>()(
  persist(
    set => ({
      layout: 1,
      panes: [null, null, null, null],
      over: -1,
      drag: null,

      setLayout: n =>
        set(s => {
          const panes = s.panes.slice();
          for (let i = n; i < 4; i++) panes[i] = null;
          return { layout: n, panes };
        }),

      dropChat: (paneIndex, chatId) =>
        set(s => {
          const panes = s.panes.map(p => (p === chatId ? null : p));
          panes[paneIndex] = chatId;
          return { panes, over: -1, drag: null };
        }),

      closePane: paneIndex =>
        set(s => {
          const panes = s.panes.slice();
          panes[paneIndex] = null;
          return { panes };
        }),

      evictChat: chatId =>
        set(s => ({ panes: s.panes.map(p => (p === chatId ? null : p)) })),

      autoPlace: chatId =>
        set(s => {
          if (s.panes.includes(chatId)) return s;
          const panes = s.panes.slice();
          const free = panes.findIndex((x, i) => i < s.layout && !x);
          if (free >= 0) panes[free] = chatId;
          return { panes };
        }),

      setOver: i => set({ over: i }),
      setDrag: id => set({ drag: id })
    }),
    { name: 'llm-desktop.panes', partialize: s => ({ layout: s.layout, panes: s.panes }) }
  )
);
