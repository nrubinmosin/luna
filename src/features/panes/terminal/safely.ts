import { logWarn } from '../../../shared/lib/log';

// xterm's webgl renderer and the fit addon both throw if they are poked while
// the host element is detached or already torn down. Any throw here escapes a
// React effect cleanup and kills the entire root, so every call is guarded.
export const safely = (fn: () => void) => {
  try {
    fn();
  } catch (e) {
    // With the stack: the message alone ("cannot read _isDisposed") names an
    // xterm internal and not which of the half-dozen guarded calls reached it.
    const stack = (e as Error)?.stack;
    logWarn('terminal', `teardown step failed: ${stack ?? String(e)}`);
  }
};
