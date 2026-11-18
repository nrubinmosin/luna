import { describe, expect, it } from 'vitest';
import type { MoveStatusDto } from '../../ipc/moving';
import { useMoveHints } from './moveHints.store';

const move = (newId: string, phase = 'opening'): MoveStatusDto =>
  ({
    moveId: `m-${newId}`,
    chatId: 'old',
    newId,
    phase,
    at: 1,
    stopOld: false,
    worktree: false,
    toAccount: 'other',
    source: 'luna'
  }) as MoveStatusDto;

describe('moveHints store: the poll’s moves', () => {
  it('keeps the same record when a read brings nothing new', () => {
    useMoveHints.getState().setMoves([move('a'), move('b')]);
    const before = useMoveHints.getState();
    useMoveHints.getState().setMoves([move('a'), move('b')]);
    expect(useMoveHints.getState()).toBe(before);
  });

  it('takes a change, keeping the moves that did not change', () => {
    useMoveHints.getState().setMoves([move('a'), move('b')]);
    const a = useMoveHints.getState().opening.a;
    useMoveHints.getState().setMoves([move('a'), move('b', 'done')]);
    expect(useMoveHints.getState().opening.a).toBe(a);
    expect(useMoveHints.getState().opening.b?.phase).toBe('done');
    useMoveHints.getState().setMoves([move('a')]);
    expect(Object.keys(useMoveHints.getState().opening)).toEqual(['a']);
  });
});
