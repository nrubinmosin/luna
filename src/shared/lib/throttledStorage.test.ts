import { afterEach, describe, expect, it, vi } from 'vitest';
import { flushPendingWrites, throttledStorage } from './throttledStorage';

const value = (n: number) => ({ state: { n }, version: 0 });
const stored = () => JSON.parse(localStorage.getItem('t') ?? 'null') as { state: { n: number } } | null;

afterEach(() => {
  vi.useRealTimers();
});

describe('throttledStorage', () => {
  it('writes a lone change at once and holds what follows for the gap', () => {
    vi.useFakeTimers();
    const s = throttledStorage<{ n: number }>(JSON.stringify);
    s.setItem('t', value(1));
    expect(stored()?.state.n).toBe(1);
    s.setItem('t', value(2));
    expect(stored()?.state.n).toBe(1);
    vi.advanceTimersByTime(500);
    expect(stored()?.state.n).toBe(2);
  });

  it('starts no gap on a write it skipped as the same text', () => {
    vi.useFakeTimers();
    const s = throttledStorage<{ n: number }>(JSON.stringify);
    s.setItem('t', value(1));
    vi.advanceTimersByTime(600);
    // The same state again writes nothing, and must not hold back the next.
    s.setItem('t', value(1));
    s.setItem('t', value(2));
    expect(stored()?.state.n).toBe(2);
  });

  it('writes what it holds back on flush, and on the way out', () => {
    vi.useFakeTimers();
    const s = throttledStorage<{ n: number }>(JSON.stringify);
    s.setItem('t', value(1));
    s.setItem('t', value(2));
    s.flush();
    expect(stored()?.state.n).toBe(2);
    s.setItem('t', value(3));
    expect(stored()?.state.n).toBe(2);
    flushPendingWrites();
    expect(stored()?.state.n).toBe(3);
  });
});
