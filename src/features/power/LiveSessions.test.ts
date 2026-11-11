import { describe, expect, it } from 'vitest';
import type { SessionActivityDto } from '../../ipc/power';
import { doing } from './LiveSessions';

const session = (s: Partial<SessionActivityDto>): SessionActivityDto => ({
  id: 'c',
  turn: 'idle',
  procs: [],
  outputFresh: false,
  busy: true,
  stalled: null,
  ...s
});

describe('doing', () => {
  it('calls what runs between turns background', () => {
    expect(doing(session({ procs: ['1 subagent', 'node (pid 7)'] }))).toBe('background: 1 subagent, node (pid 7)');
  });

  it('calls what runs during a turn running', () => {
    expect(doing(session({ turn: 'busy', procs: ['node (pid 7)'] }))).toBe('mid-turn, running node (pid 7)');
  });
});
