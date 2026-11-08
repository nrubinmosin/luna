import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { CoreStateDto } from '../../ipc/coreProcess';

/** The core's events as the store subscribed to them: fire one through `handlers`. */
const events = vi.hoisted(() => {
  const handlers = new Map<string, (payload: unknown) => void>();
  const on = (name: string) => (cb: (payload: unknown) => void) => {
    handlers.set(name, cb);
    return Promise.resolve(() => handlers.delete(name));
  };
  return { handlers, on };
});

vi.mock('../../ipc/events', () => ({
  onCoreState: events.on('core://state'),
  onStartOver: events.on('pty://start-over')
}));
vi.mock('../../ipc/coreProcess', () => ({
  coreState: vi.fn(),
  coreRestart: vi.fn()
}));
// xterm cannot run in jsdom; what matters is that every terminal is let go.
vi.mock('../panes/terminals', () => ({
  forgetAll: vi.fn()
}));

const { coreState } = await import('../../ipc/coreProcess');
const { forgetAll } = await import('../panes/terminals');
const { useCore } = await import('./core.store');

const up = (pid: number): CoreStateDto => ({ state: 'up', core: { pid } as never, error: null });

describe('core.store: when the panes start their sessions over', () => {
  /** Up on core 1, as far as the store knows; what came before is not the test's. */
  let epoch = 0;
  beforeEach(async () => {
    vi.mocked(coreState).mockResolvedValue(up(1));
    await useCore.getState().init();
    vi.mocked(forgetAll).mockClear();
    epoch = useCore.getState().epoch;
  });

  it('starts over when a new core replaces one that stopped, not on a state change alone', () => {
    events.handlers.get('core://state')!(up(1));
    expect(useCore.getState().epoch).toBe(epoch);
    expect(forgetAll).not.toHaveBeenCalled();
    events.handlers.get('core://state')!({ state: 'down', core: null, error: null });
    events.handlers.get('core://state')!(up(2));
    expect(useCore.getState().epoch).toBe(epoch + 1);
    expect(forgetAll).toHaveBeenCalledTimes(1);
  });

  it('starts over when the core says every session was closed for a shutdown that was taken back', () => {
    events.handlers.get('pty://start-over')!({ why: 'the shutdown was taken back' });
    expect(useCore.getState().epoch).toBe(epoch + 1);
    expect(forgetAll).toHaveBeenCalledTimes(1);
    expect(useCore.getState().link.state).toBe('up');
  });
});
