import type { PersistStorage, StorageValue } from 'zustand/middleware';

/**
 * localStorage for a persisted store that changes often, written at most once
 * per `gapMs`. zustand's persist hands its storage the whole state on every
 * set, and the stock JSON storage stringifies and writes it each time: for
 * the chats, a status or context change every few seconds per chat meant the
 * whole list serialized and written as often.
 *
 * A change after a quiet spell is serialized and written at once, so a lone
 * change (a rename) is on disk straight away; what follows within `gapMs`
 * waits and goes out as one write of the latest state. `serialize` turns the
 * state into what is stored, and the gap runs from the last time it ran: a
 * write of the same text as the last is skipped but still starts the gap, or
 * a burst of changes that store the same (the chats' context ticks, left out
 * of what is stored) serialized the whole state once per change. `flush`
 * writes what waits now: for a change a window started over after a hard exit
 * could not do without (a new chat, its session id), which the store calls
 * right after its set. A page going away writes what waits, and so does
 * Luna's own way out (`exiting`), after which every change is written at
 * once.
 */
export function throttledStorage<S>(
  serialize: (value: StorageValue<S>) => string,
  gapMs = 500
): PersistStorage<S> & { flush: () => void } {
  let pending: { name: string; value: StorageValue<S> } | null = null;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let lastAt = -Infinity;
  let lastText: string | null = null;

  const write = () => {
    clearTimeout(timer);
    timer = undefined;
    const p = pending;
    pending = null;
    if (!p) return;
    lastAt = Date.now();
    const text = serialize(p.value);
    if (text === lastText) return;
    lastText = text;
    try {
      localStorage.setItem(p.name, text);
    } catch {
      // Storage full or gone: the next write tries again.
      lastText = null;
    }
  };

  registry.add({
    flush: write,
    drop: () => {
      clearTimeout(timer);
      timer = undefined;
      pending = null;
      lastAt = -Infinity;
      lastText = null;
    }
  });
  if (typeof window !== 'undefined') {
    window.addEventListener('pagehide', write);
    document.addEventListener('visibilitychange', () => {
      if (document.visibilityState === 'hidden') write();
    });
  }

  return {
    flush: write,
    getItem: name => {
      // Read to hydrate the store, which takes what is stored over what it
      // held: a write still waiting would only put the old state back.
      if (pending?.name === name) {
        clearTimeout(timer);
        timer = undefined;
        pending = null;
      }
      const text = localStorage.getItem(name);
      return text == null ? null : (JSON.parse(text) as StorageValue<S>);
    },
    setItem: (name, value) => {
      pending = { name, value };
      const wait = lastAt + gapMs - Date.now();
      if (wait <= 0 || shared.exiting) write();
      else timer ??= setTimeout(write, wait);
    },
    removeItem: name => {
      if (pending?.name === name) pending = null;
      lastText = null;
      localStorage.removeItem(name);
    }
  };
}

type Throttled = { flush: () => void; drop: () => void };
/** Every throttled storage made, and whether Luna is on its way out, on
 *  globalThis so a module loaded afresh in a test (vi.resetModules) shares
 *  them. */
const shared: { all: Set<Throttled>; exiting: boolean } = ((
  globalThis as { lunaThrottledStorages?: { all: Set<Throttled>; exiting: boolean } }
).lunaThrottledStorages ??= { all: new Set(), exiting: false });
const registry = shared.all;

/** Writes what every throttled storage holds back now. */
export function flushPendingWrites() {
  registry.forEach(s => s.flush());
}

/**
 * Luna is on its way out without the page going away first (a tray Quit or
 * Restart, an update's install), and the process may end before any gap is
 * over: what every throttled storage holds is written now, and every change
 * from here on at once. `false` takes it back, for a way out that did not
 * happen (a Restart that could not start the new window, a failed install).
 */
export function exiting(on = true) {
  shared.exiting = on;
  if (on) flushPendingWrites();
}

/** For tests: forget every write still waiting, as a page that never was. */
export function dropPendingWrites() {
  shared.exiting = false;
  registry.forEach(s => s.drop());
}
