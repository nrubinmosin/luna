import { describe, expect, it } from 'vitest';
import { mapStatus } from './useSessionWatch';

describe('mapStatus', () => {
  it('takes the core’s word for work beside the turn as it is', () => {
    expect(mapStatus('background')).toBe('background');
  });

  it('folds the CLIs’ words as ever', () => {
    expect(mapStatus('busy')).toBe('working');
    expect(mapStatus('working')).toBe('working');
    expect(mapStatus('waiting')).toBe('waiting');
    expect(mapStatus('needs permission')).toBe('waiting');
    expect(mapStatus('idle')).toBe('resting');
    expect(mapStatus(null)).toBe('resting');
  });
});
