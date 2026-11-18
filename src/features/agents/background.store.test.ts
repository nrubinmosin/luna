import { describe, expect, it } from 'vitest';
import type { BackgroundHelperDto } from '../../ipc/agents';
import { helperRoots, helpersUnder } from './background.store';
import { namesThatFit } from './HelperPill';

const helper = (id: string, parent: string | null) => ({ id, parent }) as BackgroundHelperDto;

describe('background store: what each helper hangs from', () => {
  it('files helpers under the chat at the top of their line, once per snapshot', () => {
    const all = [
      helper('h1', 'chat'),
      helper('h2', 'h1'),
      helper('h3', 'gone'),
      helper('h4', null),
      helper('h5', 'h2')
    ];
    const index = helperRoots(all);
    expect(index.get('chat')!.map(h => h.id)).toEqual(['h1', 'h2', 'h5']);
    expect(index.get('gone')!.map(h => h.id)).toEqual(['h3']);
    expect(index.get(null)!.map(h => h.id)).toEqual(['h4']);
    expect(helperRoots(all)).toBe(index);
    expect(helpersUnder(all, 'chat')).toBe(helpersUnder(all, 'chat'));
    expect(helpersUnder(all, 'nobody')).toEqual([]);
  });

  it('stops on a loop rather than walking it forever', () => {
    const all = [helper('a', 'b'), helper('b', 'a')];
    expect([...helperRoots(all).values()].flat()).toHaveLength(2);
  });
});

describe('HelperPill: which names fit', () => {
  it('drops names from the end, and the list when not even the first fits', () => {
    // A pill 200 wide with three names of 40, 30 and 50, 8 apart.
    expect(namesThatFit(200, [40, 30, 50], 200)).toBe(3);
    expect(namesThatFit(200, [40, 30, 50], 150)).toBe(2);
    expect(namesThatFit(200, [40, 30, 50], 112)).toBe(1);
    expect(namesThatFit(200, [40, 30, 50], 60)).toBe(0);
    expect(namesThatFit(80, [], 50)).toBe(0);
  });
});
