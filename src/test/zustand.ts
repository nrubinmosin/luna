// zustand's recommended testing setup (docs: "Testing"): every store made
// through `create` or `createStore` registers a way back to its initial state,
// and setup.ts runs them all after each test, so no test sees another's state.
// The registry lives on globalThis because a test that re-imports a module
// (vi.resetModules, to read localStorage at load) gets a fresh copy of this
// one as well, and its stores must be reset all the same.
import { vi } from 'vitest';
import type * as Zustand from 'zustand';

export * from 'zustand';

const actual = await vi.importActual<typeof Zustand>('zustand');

type Registry = { zustandResets?: Set<() => void> };
export const storeResets: Set<() => void> = ((globalThis as Registry).zustandResets ??= new Set());

const track = <S extends { getInitialState: () => unknown; setState: (s: never, replace: true) => void }>(store: S) => {
  const initial = store.getInitialState();
  storeResets.add(() => store.setState(initial as never, true));
  return store;
};

const createUncurried = (creator: Zustand.StateCreator<unknown>) => track(actual.create(creator));
export const create = ((creator?: Zustand.StateCreator<unknown>) =>
  creator ? createUncurried(creator) : createUncurried) as typeof Zustand.create;

const createStoreUncurried = (creator: Zustand.StateCreator<unknown>) => track(actual.createStore(creator));
export const createStore = ((creator?: Zustand.StateCreator<unknown>) =>
  creator ? createStoreUncurried(creator) : createStoreUncurried) as typeof Zustand.createStore;
