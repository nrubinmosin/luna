import { onPtyExit, onPtyInputRefused, onPtyOutput } from '../../../ipc/events';

/** What one session's terminal takes from its pty. */
export interface PtyRoute {
  output: (data: string) => void;
  /** `generation`: the spawn number of the session that exited (`PtyExit`). */
  exit: (code: number | null, generation: number) => void;
  /** The session refused keys, `waitingKb` of them waiting already. */
  refused?: (waitingKb: number) => void;
}

/**
 * Every pty's output, exit and refused input reach the window as Tauri
 * events, whatever the session. Each terminal used to listen to them all and
 * drop what was not its own, so every chunk of output ran through every
 * terminal's listener; here one listener per event hands each payload to the
 * routes of its session.
 *
 * The listeners are made on the first route and kept for the page's life.
 * One that fails to register is tried again by the next route, and the
 * others, already in, are not registered twice.
 */
const routes = new Map<string, Set<PtyRoute>>();
let outputIn: Promise<void> | null = null;
let exitIn: Promise<void> | null = null;
let refusedIn: Promise<void> | null = null;

function listening(): Promise<void> {
  outputIn ??= onPtyOutput(p => routes.get(p.id)?.forEach(r => r.output(p.data))).then(
    () => {},
    (e: unknown) => {
      outputIn = null;
      throw e;
    }
  );
  exitIn ??= onPtyExit(p => routes.get(p.id)?.forEach(r => r.exit(p.code, p.generation))).then(
    () => {},
    (e: unknown) => {
      exitIn = null;
      throw e;
    }
  );
  // Refused input only adds a notice to a pane, and sign-in takes none, so a
  // failure here holds no route back: it is logged and tried again by the
  // next route.
  refusedIn ??= onPtyInputRefused(p => routes.get(p.id)?.forEach(r => r.refused?.(p.waitingKb))).then(
    () => {},
    (e: unknown) => {
      refusedIn = null;
      console.warn('[luna] pty input-refused listener failed', e);
    }
  );
  return Promise.all([outputIn, exitIn, refusedIn]).then(() => {});
}

/**
 * Sends session `id`'s output, exit and refused input to `route` from now
 * until `unroute`. `ready` settles once every listener is in: a session
 * started before then could print what nobody hears. It rejects if the output
 * or exit listener failed to register, not the refused-input one; the route
 * stays until `unroute` all the same.
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
