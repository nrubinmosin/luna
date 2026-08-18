import { invoke } from '@tauri-apps/api/core';

const tauriAvailable = '__TAURI_INTERNALS__' in window;

type Level = 'info' | 'warn' | 'error';

const send = (level: Level, source: string, message: string) => {
  const line = `[${source}] ${message}`;
  if (level === 'error') console.error(line);
  else if (level === 'warn') console.warn(line);
  else console.info(line);
  // Logging must never be able to break the caller.
  if (tauriAvailable) invoke('append_log', { level, source, message }).catch(() => {});
};

export const logInfo = (source: string, message: string) => send('info', source, message);
export const logWarn = (source: string, message: string) => send('warn', source, message);
export const logError = (source: string, message: string) => send('error', source, message);

/** Anything above this is long enough for the window to feel stuck. */
export const SLOW_MS = 400;

/**
 * Watches for the UI thread going away. A timer that should fire every second
 * but fires late by a lot means the main thread was blocked for that long —
 * which is what a "the app froze for a couple of seconds" report looks like
 * from the inside. Pairs with the per-command timings: if a slow command is
 * logged next to a stall, they are the same event.
 */
export function watchMainThreadStalls(thresholdMs = 900) {
  let last = performance.now();
  setInterval(() => {
    const now = performance.now();
    const drift = now - last - 1000;
    last = now;
    if (drift > thresholdMs) {
      logWarn('stall', `UI thread blocked for ~${Math.round(drift)}ms`);
    }
  }, 1000);
}

/** Reports unhandled failures that would otherwise only reach devtools. */
export function installGlobalErrorLog() {
  window.addEventListener('error', e => {
    logError('window', `${e.message} (${e.filename}:${e.lineno})`);
  });
  window.addEventListener('unhandledrejection', e => {
    logError('promise', String((e.reason as Error)?.stack ?? e.reason));
  });
}
