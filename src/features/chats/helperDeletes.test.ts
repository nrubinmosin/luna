import { describe, expect, it } from 'vitest';
import type { BackgroundHelperDto } from '../../ipc/agents';
import { helperDeletes, helperLines, type HelperChoices } from './helperDeletes';

const row = (id: string, state: string, over: Partial<BackgroundHelperDto> = {}): BackgroundHelperDto => ({
  id,
  parent: 'chat',
  name: id,
  provider: 'claude',
  state,
  account: 'work',
  model: null,
  effort: 'low',
  worktree: true,
  startedMs: 0,
  turnStartedMs: null,
  lastActivityMs: null,
  task: null,
  ...over
});

const all: HelperChoices = { withHelpers: true, dropWorktree: true, stopWorking: false, dropUnsaved: false };

describe('deleting a chat with helpers', () => {
  const helpers = [
    row('done', 'idle'),
    row('busy', 'working'),
    row('asks', 'question'),
    row('codex', 'idle', { provider: 'codex' })
  ];
  const stake = {
    done: { id: 'done', worktree: '/wt/done', unsaved: '2 uncommitted files' },
    codex: { id: 'codex', worktree: '/wt/codex' },
    // The core's look found a job left running that the row had not shown yet.
    asks: { id: 'asks', working: 'waits on a question or permission' }
  };
  const lines = helperLines(helpers, stake);

  it('lists what each one is doing and what its worktree holds', () => {
    expect(lines).toEqual([
      { id: 'done', name: 'done', working: null, unsaved: '/wt/done: 2 uncommitted files' },
      { id: 'busy', name: 'busy', working: 'working', unsaved: null },
      { id: 'asks', name: 'asks', working: 'waits on a question or permission', unsaved: null },
      { id: 'codex', name: 'codex', working: null, unsaved: null }
    ]);
    // Before the core has looked, the row's state alone.
    expect(helperLines(helpers, null).map(l => l.working)).toEqual([null, 'working', 'asks a question', null]);
  });

  it('touches none of them unless asked', () => {
    expect(helperDeletes(lines, { ...all, withHelpers: false })).toEqual([]);
  });

  it('never stops one at work or drops unsaved work on the chat’s boxes alone', () => {
    expect(helperDeletes(lines, all)).toEqual([
      { id: 'done', dropWorktree: true, stopWorking: false, dropUnsaved: false },
      { id: 'codex', dropWorktree: true, stopWorking: false, dropUnsaved: false }
    ]);
  });

  it('stops the working ones and drops unsaved work each only on its own yes', () => {
    const stopped = helperDeletes(lines, { ...all, stopWorking: true });
    expect(stopped.map(d => [d.id, d.stopWorking])).toEqual([
      ['done', false],
      ['busy', true],
      ['asks', true],
      ['codex', false]
    ]);
    const dropped = helperDeletes(lines, { ...all, dropUnsaved: true });
    expect(dropped.find(d => d.id === 'done')?.dropUnsaved).toBe(true);
    // Worktrees kept: nothing to drop, unsaved or not.
    expect(
      helperDeletes(lines, { ...all, dropWorktree: false, dropUnsaved: true }).every(
        d => !d.dropUnsaved && !d.dropWorktree
      )
    ).toBe(true);
  });
});
