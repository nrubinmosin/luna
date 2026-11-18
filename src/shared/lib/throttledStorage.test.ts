import { afterEach, describe, expect, it, vi } from 'vitest';
import { exiting, flushPendingWrites, throttledStorage } from './throttledStorage';

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

  it('serializes a burst of changes that store the same text once per gap', () => {
    vi.useFakeTimers();
    const serialize = vi.fn(JSON.stringify);
    const s = throttledStorage<{ n: number }>(serialize);
    s.setItem('t', value(1));
    vi.advanceTimersByTime(600);
    serialize.mockClear();
    // As a context tick of every chat: a new state each time, the same text.
    for (let i = 0; i < 20; i++) s.setItem('t', value(1));
    expect(serialize).toHaveBeenCalledTimes(1);
    vi.advanceTimersByTime(500);
    expect(serialize).toHaveBeenCalledTimes(2);
    // A change after it waits for the gap the skipped write started, and a
    // flush (a new chat, a session id) still writes it at once.
    s.setItem('t', value(2));
    expect(stored()?.state.n).toBe(1);
    s.flush();
    expect(stored()?.state.n).toBe(2);
  });

  it('writes every change at once while Luna is on its way out, until that is taken back', () => {
    vi.useFakeTimers();
    const s = throttledStorage<{ n: number }>(JSON.stringify);
    s.setItem('t', value(1));
    s.setItem('t', value(2));
    exiting('tray');
    expect(stored()?.state.n).toBe(2);
    // A title the watcher sets after the tray's Quit was heard.
    s.setItem('t', value(3));
    expect(stored()?.state.n).toBe(3);
    exiting('tray', false);
    s.setItem('t', value(4));
    expect(stored()?.state.n).toBe(3);
    vi.advanceTimersByTime(500);
    expect(stored()?.state.n).toBe(4);
  });

  it('keeps writing at once until every way out under way is taken back', () => {
    vi.useFakeTimers();
    const s = throttledStorage<{ n: number }>(JSON.stringify);
    s.setItem('t', value(1));
    // The tray's Restart is under way when an update's install fails.
    exiting('tray');
    exiting('update');
    exiting('update', false);
    s.setItem('t', value(2));
    expect(stored()?.state.n).toBe(2);
    exiting('tray', false);
    s.setItem('t', value(3));
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
