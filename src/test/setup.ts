import '@testing-library/jest-dom/vitest';
import { act, cleanup } from '@testing-library/react';
import { afterEach, vi } from 'vitest';
import { storeResets } from './zustand';

// Every store starts each test as it was made (see ./zustand).
vi.mock('zustand', () => import('./zustand'));

// Vitest runs without globals, so Testing Library cannot register its own
// cleanup: each test unmounts what it rendered here. Then the stores go back
// to their first state, and what persisting them wrote goes with it.
afterEach(() => {
  cleanup();
  act(() => storeResets.forEach(reset => reset()));
  localStorage.clear();
});
