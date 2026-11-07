import { beforeEach, describe, expect, it, vi } from 'vitest';

/** The store as a fresh start of the app finds it: it reads localStorage as it loads. */
async function load() {
  vi.resetModules();
  const { useShell } = await import('./shell.store');
  return useShell;
}

beforeEach(() => localStorage.clear());

describe('shell store: theme', () => {
  it('follows the system until a theme is picked', async () => {
    const shell = await load();
    expect(shell.getState().theme).toBe('system');
  });

  it('cycles system → light → dark → system', async () => {
    const shell = await load();
    const seen = [1, 2, 3].map(() => {
      shell.getState().cycleTheme();
      return shell.getState().theme;
    });
    expect(seen).toEqual(['light', 'dark', 'system']);
  });

  it('remembers the theme across a restart', async () => {
    (await load()).getState().cycleTheme();
    expect((await load()).getState().theme).toBe('light');
  });
});

describe('shell store: sidebar', () => {
  it('starts pinned', async () => {
    expect((await load()).getState().pinned).toBe(true);
  });

  it('remembers being unpinned across a restart', async () => {
    (await load()).getState().setPinned(false);
    expect((await load()).getState().pinned).toBe(false);
  });

  it('pinning puts away the slid-out overlay', async () => {
    const shell = await load();
    shell.getState().setPinned(false);
    shell.getState().setOverlay(true);
    shell.getState().setPinned(true);
    expect(shell.getState().overlay).toBe(false);
  });

  it('keeps the width between 220 and 480', async () => {
    const shell = await load();
    shell.getState().setWidth(100);
    expect(shell.getState().width).toBe(220);
    shell.getState().setWidth(900);
    expect(shell.getState().width).toBe(480);
    shell.getState().setWidth(333);
    expect(shell.getState().width).toBe(333);
  });

  it('does not remember the width across a restart', async () => {
    (await load()).getState().setWidth(400);
    expect((await load()).getState().width).toBe(300);
  });
});

describe('shell store: a blocked localStorage', () => {
  it('still starts, and still switches the theme, only without remembering it', async () => {
    const get = vi.spyOn(Storage.prototype, 'getItem').mockImplementation(() => {
      throw new Error('blocked');
    });
    const set = vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
      throw new Error('blocked');
    });
    try {
      const shell = await load();
      expect(shell.getState().theme).toBe('system');
      expect(shell.getState().pinned).toBe(true);
      shell.getState().cycleTheme();
      expect(shell.getState().theme).toBe('light');
    } finally {
      get.mockRestore();
      set.mockRestore();
    }
  });
});
