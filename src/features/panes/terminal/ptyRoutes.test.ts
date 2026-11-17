import { beforeEach, describe, expect, it, vi } from 'vitest';

/** The pty's two events, each listener kept; `fail` makes the next listen
 *  of that event reject. */
const events = vi.hoisted(() => {
  const output: Array<(p: { id: string; data: string }) => void> = [];
  const exit: Array<(p: { id: string; code: number | null }) => void> = [];
  const fail = { output: false, exit: false };
  const listen =
    <T>(list: Array<(p: T) => void>, which: 'output' | 'exit') =>
    (cb: (p: T) => void) => {
      if (fail[which]) {
        fail[which] = false;
        return Promise.reject(new Error(`no ${which}`));
      }
      list.push(cb);
      return Promise.resolve(() => {});
    };
  return { output, exit, fail, onPtyOutput: listen(output, 'output'), onPtyExit: listen(exit, 'exit') };
});

vi.mock('../../../ipc/events', () => ({ onPtyOutput: events.onPtyOutput, onPtyExit: events.onPtyExit }));

/** The router as a page that has not listened yet finds it. */
async function freshRouter() {
  vi.resetModules();
  return (await import('./ptyRoutes')).routePty;
}

const recorder = () => {
  const got: string[] = [];
  return { got, route: { output: (d: string) => got.push(d), exit: (c: number | null) => got.push(`exit ${c}`) } };
};

beforeEach(() => {
  events.output.length = 0;
  events.exit.length = 0;
  events.fail.output = false;
  events.fail.exit = false;
});

describe('ptyRoutes', () => {
  it('listens once per event however many sessions it routes, and hands each its own', async () => {
    const routePty = await freshRouter();
    const a = recorder();
    const b = recorder();
    await routePty('a', a.route).ready;
    await routePty('b', b.route).ready;
    expect(events.output).toHaveLength(1);
    expect(events.exit).toHaveLength(1);

    events.output[0]!({ id: 'a', data: 'for a' });
    events.output[0]!({ id: 'b', data: 'for b' });
    events.exit[0]!({ id: 'a', code: 0 });
    events.output[0]!({ id: 'nobody', data: 'dropped' });
    expect(a.got).toEqual(['for a', 'exit 0']);
    expect(b.got).toEqual(['for b']);
  });

  it('sends nothing to a route taken down', async () => {
    const routePty = await freshRouter();
    const a = recorder();
    const r = routePty('a', a.route);
    await r.ready;
    r.unroute();
    events.output[0]!({ id: 'a', data: 'late' });
    expect(a.got).toEqual([]);
  });

  it('retries a listen that failed with the next route, and registers the other only once', async () => {
    const routePty = await freshRouter();
    events.fail.exit = true;
    const first = routePty('a', recorder().route);
    await expect(first.ready).rejects.toThrow('no exit');
    expect(events.output).toHaveLength(1);
    expect(events.exit).toHaveLength(0);

    const b = recorder();
    await routePty('b', b.route).ready;
    expect(events.output).toHaveLength(1);
    expect(events.exit).toHaveLength(1);
    events.exit[0]!({ id: 'b', code: 1 });
    expect(b.got).toEqual(['exit 1']);
  });
});
