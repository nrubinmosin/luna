import { describe, expect, it } from 'vitest';
import type { BackgroundHelperDto } from '../../ipc/agents';
import {
  helperDeletes,
  helperLines,
  helperMoves,
  movingWith,
  preselectPass,
  type HelperChoices
} from './helperDeletes';

const row = (id: string, state: string, over: Partial<BackgroundHelperDto> = {}): BackgroundHelperDto => ({
  id,
  parent: 'chat',
  name: id,
  provider: 'claude',
  state,
  account: 'work',
  model: null,
  effort: 'low',
  startedMs: 0,
  turnStartedMs: null,
  lastActivityMs: null,
  task: null,
  ...over
});

const all: HelperChoices = { withHelpers: true, stopWorking: false };

describe('deleting a chat with helpers', () => {
  const helpers = [
    row('done', 'idle'),
    row('busy', 'working'),
    row('asks', 'question'),
    row('codex', 'idle', { provider: 'codex' })
  ];
  const stake = {
    // The core's look found a job left running that the row had not shown yet.
    asks: { id: 'asks', working: 'waits on a question or permission' }
  };
  const lines = helperLines(helpers, stake);

  it('lists what each one is doing', () => {
    expect(lines).toEqual([
      { id: 'done', name: 'done', parent: 'chat', working: null },
      { id: 'busy', name: 'busy', parent: 'chat', working: 'working' },
      { id: 'asks', name: 'asks', parent: 'chat', working: 'waits on a question or permission' },
      { id: 'codex', name: 'codex', parent: 'chat', working: null }
    ]);
    // Before the core has looked, the row's state alone.
    expect(helperLines(helpers, null).map(l => l.working)).toEqual([null, 'working', 'asks a question', null]);
  });

  it('touches none of them unless asked', () => {
    expect(helperDeletes(lines, { ...all, withHelpers: false })).toEqual([]);
  });

  it('never stops one at work on the chat’s box alone', () => {
    expect(helperDeletes(lines, all)).toEqual([
      { id: 'done', stopWorking: false },
      { id: 'codex', stopWorking: false }
    ]);
  });

  it('stops the working ones only on a yes of its own', () => {
    const stopped = helperDeletes(lines, { ...all, stopWorking: true });
    expect(stopped.map(d => [d.id, d.stopWorking])).toEqual([
      ['done', false],
      ['busy', true],
      ['asks', true],
      ['codex', false]
    ]);
  });
});

describe('passing a chat’s helpers to another chat as it is deleted', () => {
  const helpers = [row('lead', 'working'), row('kid', 'idle', { parent: 'lead' }), row('solo', 'idle')];
  const lines = helperLines(helpers, null);

  it('moves a passed helper with the helpers under it, and deletes none of them', () => {
    const moving = movingWith(lines, { lead: 'docs' });
    expect([...moving]).toEqual(['lead', 'kid']);
    expect(helperMoves(lines, { lead: 'docs' })).toEqual([{ id: 'lead', name: 'lead', to: 'docs' }]);
    const deletes = helperDeletes(lines, { ...all, stopWorking: true }, moving);
    expect(deletes.map(d => d.id)).toEqual(['solo']);
    expect(movingWith(lines, {}).size).toBe(0);
  });

  it('starts on the chat that last asked for it, else one waiting on it, if it may go there', () => {
    const targets = {
      targets: [
        { id: 'docs', ok: true },
        { id: 'ops', ok: true },
        { id: 'bare', ok: false, why: 'runs without Luna’s tools' }
      ]
    };
    const asked = row('h', 'idle', {
      suggested: [
        { chat: 'ops', atMs: 1 },
        { chat: 'docs', atMs: 2 }
      ],
      watchedBy: ['ops']
    });
    expect(preselectPass(asked, targets)).toBe('docs');
    expect(preselectPass(row('h', 'idle', { watchedBy: ['bare', 'ops'] }), targets)).toBe('ops');
    expect(preselectPass(row('h', 'idle', { suggested: [{ chat: 'bare', atMs: 1 }] }), targets)).toBeNull();
    expect(preselectPass(asked, { blocked: 'held back', targets: targets.targets })).toBeNull();
    expect(preselectPass(asked, undefined)).toBeNull();
    expect(preselectPass(undefined, targets)).toBeNull();
    // By when it asked, not where in the list: the core moves a chat's time on
    // in place when it asks again.
    const again = row('h', 'idle', {
      suggested: [
        { chat: 'docs', atMs: 5 },
        { chat: 'ops', atMs: 2 }
      ]
    });
    expect(preselectPass(again, targets)).toBe('docs');
  });

  it('deletes only what the dialog showed, of what is left', () => {
    const lines = helperLines([row('shown', 'idle'), row('spawned since', 'idle')], null);
    expect(helperDeletes(lines, all, new Set(), new Set(['shown'])).map(d => d.id)).toEqual(['shown']);
  });
});
