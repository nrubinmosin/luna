import { create } from 'zustand';

export type ThemePref = 'system' | 'light' | 'dark';

/** The width an unpinned sidebar keeps in the row: none — the hover band
 *  lies over the board's margin. */
export const STRIP = 0;

const read = (key: string) => {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
};
const write = (key: string, value: string) => {
  try {
    localStorage.setItem(key, value);
  } catch {
    // a blocked localStorage only forgets the choice
  }
};

/**
 * The window's own furniture: the theme, and whether the sidebar is pinned
 * open. Both are choices about what the window is for, so they outlive the
 * run; the sidebar's width is per session. Lives here rather than in App so
 * the title bar's buttons can reach it.
 */
interface ShellState {
  theme: ThemePref;
  /** Pinned: the sidebar holds its place in the row. Unpinned: it leaves a
   *  strip, and hovering the strip slides it out over the board. */
  pinned: boolean;
  width: number;
  /** Unpinned and slid out over the board by the hover. */
  overlay: boolean;
  cycleTheme: () => void;
  setPinned: (v: boolean) => void;
  setWidth: (w: number) => void;
  setOverlay: (v: boolean) => void;
}

export const useShell = create<ShellState>(set => ({
  theme: (read('luna.theme') as ThemePref) || 'system',
  pinned: read('luna.sidebar') !== 'off',
  width: 300,
  overlay: false,
  cycleTheme: () =>
    set(s => {
      const theme: ThemePref = s.theme === 'system' ? 'light' : s.theme === 'light' ? 'dark' : 'system';
      write('luna.theme', theme);
      return { theme };
    }),
  setPinned: pinned => {
    write('luna.sidebar', pinned ? 'on' : 'off');
    set({ pinned, overlay: false });
  },
  setOverlay: overlay => set({ overlay }),
  setWidth: w => set({ width: Math.min(480, Math.max(220, w)) })
}));
