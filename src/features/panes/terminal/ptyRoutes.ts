import { onPtyExit, onPtyOutput } from '../../../ipc/events';

/** What one session's terminal takes from its pty. */
export interface PtyRoute {
  output: (data: string) => void;
  exit: (code: number | null) => void;
}

/**
 * Every pty's output and exit reach the window as two Tauri events, whatever
 * the session. Each terminal used to listen to both and drop what was not its
 * own, so every chunk of output ran through every terminal's listener; here
 * one listener per event hands each payload to the routes of its session.
 *
 * The two listeners are made on the first route and kept for the page's
 * life. One that fails to register is tried again by the next route, and the
 * other, already in, is not registered twice.
 */
const routes = new Map<string, Set<PtyRoute>>();
let outputIn: Promise<void> | null = null;
let exitIn: Promise<void> | null = null;

function listening(): Promise<void> {
  outputIn ??= onPtyOutput(p => routes.get(p.id)?.forEach(r => r.output(p.data))).then(
    () => {},
    (e: unknown) => {
      outputIn = null;
      throw e;
    }
  );
  exitIn ??= onPtyExit(p => routes.get(p.id)?.forEach(r => r.exit(p.code))).then(
    () => {},
    (e: unknown) => {
      exitIn = null;
      throw e;
    }
  );
  return Promise.all([outputIn, exitIn]).then(() => {});
}

/**
 * Sends session `id`'s output and exit to `route` from now until `unroute`.
 * `ready` settles once both listeners are in: a session started before then
 * could print what nobody hears. It rejects if either failed to register; the
 * route stays until `unroute` all the same.
 */
export function routePty(id: string, route: PtyRoute): { ready: Promise<void>; unroute: () => void } {
  let set = routes.get(id);
  if (!set) routes.set(id, (set = new Set()));
  set.add(route);
  return {
    ready: listening(),
    unroute: () => {
      const now = routes.get(id);
      if (!now) return;
      now.delete(route);
      if (now.size === 0) routes.delete(id);
    }
  };
}
