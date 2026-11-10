import { describe, expect, it } from 'vitest';
import type { BackgroundHelperDto } from '../../ipc/agents';
import { countHelpers, countWords, defaultTab, helpersIn, summaryLabel, summaryNames, tabOf } from './helperSummary';

const NOW = 1_000_000_000;

const row = (id: string, state: string, over: Partial<BackgroundHelperDto> = {}): BackgroundHelperDto => ({
  id,
  parent: 'chat',
  name: id,
  provider: 'claude',
  state,
  account: 'work',
  model: null,
  effort: 'low',
  worktree: false,
  startedMs: NOW - 600_000,
  turnStartedMs: null,
  lastActivityMs: null,
  task: null,
  ...over
});

describe('a chat helpers summed up', () => {
  it('counts the asking among the working, and the gone nowhere', () => {
    const helpers = [
      row('a', 'starting'),
      row('b', 'working'),
      row('c', 'background'),
      row('d', 'question'),
      row('e', 'waiting'),
      row('f', 'idle'),
      row('g', 'exited')
    ];
    expect(countHelpers(helpers)).toEqual({ working: 5, idle: 1, asks: 2 });
    expect(tabOf(row('x', 'exited'))).toBeNull();
  });

  it('leaves zero counts out of the words', () => {
    expect(countWords({ working: 2, idle: 1, asks: 1 })).toEqual(['2 working', '1 idle', '1 asks']);
    expect(countWords({ working: 0, idle: 3, asks: 0 })).toEqual(['3 idle']);
    expect(countWords({ working: 0, idle: 0, asks: 0 })).toEqual([]);
  });

  it('names the asking first, then the working with their turn time, and no idle', () => {
    const names = summaryNames(
      [
        row('review-plan', 'working', { turnStartedMs: NOW - 4 * 60_000 }),
        row('done', 'idle'),
        row('check-changelog', 'question', { turnStartedMs: NOW - 60_000 })
      ],
      NOW
    );
    expect(names.map(n => [n.text, n.asks])).toEqual([
      ['check-changelog', true],
      ['review-plan 4m', false]
    ]);
  });

  it('says it all in the label, for a screen reader', () => {
    const label = summaryLabel([row('w', 'working'), row('q', 'question'), row('i', 'idle'), row('x', 'exited')], NOW);
    expect(label).toContain('2 working, 1 of them asking, 1 idle, 1 gone');
    expect(label).toContain('Asking: q.');
    expect(label).toContain('Working: w.');
  });
});

describe('the list of a chat helpers', () => {
  const helpers = [row('w1', 'working'), row('i1', 'idle'), row('q1', 'waiting'), row('x1', 'exited')];

  it('lists the asking at the head of Working', () => {
    expect(helpersIn(helpers, 'working').map(h => h.id)).toEqual(['q1', 'w1']);
  });

  it('lists the idle and the gone under Idle', () => {
    expect(helpersIn(helpers, 'idle').map(h => h.id)).toEqual(['i1', 'x1']);
  });

  it('opens on Working while anything works, else on Idle', () => {
    expect(defaultTab(helpers)).toBe('working');
    expect(defaultTab([row('i', 'idle'), row('x', 'exited')])).toBe('idle');
    expect(defaultTab([])).toBe('idle');
  });
});
