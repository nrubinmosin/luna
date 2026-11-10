import { describe, expect, it } from 'vitest';
import { syncAllCounts } from './places';

describe('syncAllCounts', () => {
  it('counts the copies replaced and those taken out, each only when there are some', () => {
    expect(syncAllCounts(1, 0)).toBe('1 copy changed where it lies replaced');
    expect(syncAllCounts(0, 2)).toBe('2 copies no longer meant for an account taken out');
    expect(syncAllCounts(3, 1)).toBe(
      '3 copies changed where they lie replaced, 1 copy no longer meant for an account taken out'
    );
  });
});
