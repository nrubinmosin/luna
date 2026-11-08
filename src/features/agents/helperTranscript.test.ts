import { act, renderHook } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import * as agents from '../../ipc/agents';
import type { HelperTranscriptDto } from '../../ipc/agents';
import { TAIL_IDLE_MS, TAIL_RUNNING_MS, VIEW_KEEP, useHelperTranscript } from './helperTranscript';

vi.mock('../../ipc/agents', async importOriginal => ({
  ...(await importOriginal<typeof import('../../ipc/agents')>()),
  backgroundTranscript: vi.fn()
}));
const mocked = vi.mocked(agents);

const said = (text: string, role = 'assistant') => ({ role, text });
const look = (over: Partial<HelperTranscriptDto>): HelperTranscriptDto => ({
  messages: [],
  cursor: 0,
  file: '/t/a.jsonl',
  running: true,
  ...over
});

/** Lets the pending look settle, then moves the clock by `ms`. */
async function after(ms: number) {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ms);
  });
}

beforeEach(() => {
  vi.useFakeTimers();
  mocked.backgroundTranscript.mockReset();
});
afterEach(() => vi.useRealTimers());

describe('the read-only view of a helper', () => {
  it('tails its conversation from the cursor while a turn runs, and slower between turns', async () => {
    mocked.backgroundTranscript
      .mockResolvedValueOnce(look({ messages: [said('do the job', 'user'), said('on it')], cursor: 100 }))
      .mockResolvedValueOnce(look({ messages: [said('half way')], cursor: 150 }))
      .mockResolvedValueOnce(look({ messages: [said('done')], cursor: 180, running: false }))
      .mockResolvedValue(look({ cursor: 180, running: false }));
    const { result } = renderHook(() => useHelperTranscript('bg1'));
    await after(0);
    expect(result.current.messages.map(m => m.text)).toEqual(['do the job', 'on it']);
    expect(result.current.running).toBe(true);
    expect(mocked.backgroundTranscript).toHaveBeenLastCalledWith('bg1', null, null);
    await after(TAIL_RUNNING_MS);
    expect(mocked.backgroundTranscript).toHaveBeenLastCalledWith('bg1', 100, '/t/a.jsonl');
    expect(result.current.messages.map(m => m.text)).toEqual(['do the job', 'on it', 'half way']);
    await after(TAIL_RUNNING_MS);
    expect(result.current.messages.at(-1)?.text).toBe('done');
    expect(result.current.running).toBe(false);
    // Between turns: the next look waits the longer time.
    const calls = mocked.backgroundTranscript.mock.calls.length;
    await after(TAIL_RUNNING_MS);
    expect(mocked.backgroundTranscript.mock.calls.length).toBe(calls);
    await after(TAIL_IDLE_MS);
    expect(mocked.backgroundTranscript.mock.calls.length).toBe(calls + 1);
    expect(mocked.backgroundTranscript).toHaveBeenLastCalledWith('bg1', 180, '/t/a.jsonl');
  });

  it('starts over on a new transcript file, and keeps only the newest of a long one', async () => {
    const many = Array.from({ length: VIEW_KEEP + 3 }, (_, i) => said(`m${i}`));
    mocked.backgroundTranscript
      .mockResolvedValueOnce(look({ messages: many, cursor: 10, leftOut: 7 }))
      .mockResolvedValueOnce(look({ messages: [said('fresh start')], cursor: 5, file: '/t/b.jsonl' }))
      .mockResolvedValue(look({ cursor: 5, file: '/t/b.jsonl' }));
    const { result } = renderHook(() => useHelperTranscript('bg1'));
    await after(0);
    expect(result.current.messages).toHaveLength(VIEW_KEEP);
    expect(result.current.messages[0]?.text).toBe('m3');
    expect(result.current.leftOut).toBe(10);
    await after(TAIL_RUNNING_MS);
    expect(result.current.messages.map(m => m.text)).toEqual(['fresh start']);
    expect(result.current.leftOut).toBe(0);
  });

  it('stops looking once closed, and once the helper is gone', async () => {
    mocked.backgroundTranscript.mockResolvedValue(look({ cursor: 1 }));
    const { unmount } = renderHook(() => useHelperTranscript('bg1'));
    await after(0);
    unmount();
    const calls = mocked.backgroundTranscript.mock.calls.length;
    await after(TAIL_IDLE_MS * 2);
    expect(mocked.backgroundTranscript.mock.calls.length).toBe(calls);

    mocked.backgroundTranscript.mockReset();
    mocked.backgroundTranscript.mockRejectedValue(new Error('no such helper'));
    const { result } = renderHook(() => useHelperTranscript('bg-gone'));
    await after(0);
    expect(result.current.error).toBe('no such helper');
    await after(TAIL_IDLE_MS * 2);
    expect(mocked.backgroundTranscript).toHaveBeenCalledOnce();
  });
});
