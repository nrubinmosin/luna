import type { PersistStorage, StorageValue } from 'zustand/middleware';

/**
 * localStorage for a persisted store that changes often, written at most once
 * per `gapMs`. zustand's persist hands its storage the whole state on every
 * set, and the stock JSON storage stringifies and writes it each time: for
 * the chats, a status or context change every few seconds per chat meant the
 * whole list serialized and written as often.
 *
 * A write after a quiet spell goes out at once, so a lone change (a session
 * id, a rename) is on disk straight away; what follows within `gapMs` waits
 * and goes out as one write of the latest state. `serialize` turns the state
 * into what is stored; a write of the same text as the last is skipped. A
 * page going away writes what waits.
 */
export function throttledStorage<S>(serialize: (value: StorageValue<S>) => string, gapMs = 500): PersistStorage<S> {
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
      if (wait <= 0) write();
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
/** Every throttled storage made, on globalThis so a module loaded afresh in a
 *  test (vi.resetModules) adds to the same set. */
const registry: Set<Throttled> = ((globalThis as { lunaThrottledStorages?: Set<Throttled> }).lunaThrottledStorages ??=
  new Set());

/** For tests: forget every write still waiting, as a page that never was. */
export function dropPendingWrites() {
  registry.forEach(s => s.drop());
}
