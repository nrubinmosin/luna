import { resizeSession, writeSession } from '../../../ipc/sessions';

/**
 * The pty session exists only between ensure_session resolving and pty://exit.
 * Outside that window every write/resize came straight back as "no such
 * session" — once as an ipc warning and once more as an unhandled rejection —
 * and the layout resizes a pane goes through while it is opening or after its
 * CLI exited kept both logs busy. Gate the senders on the session actually
 * being there; the .catch covers the race where the pty dies while a call is
 * in flight.
 */
export function ptySender(chatId: string) {
  let sessionLive = false;
  return {
    setLive: (on: boolean) => {
      sessionLive = on;
    },
    resize: (cols: number, rows: number) => {
      if (!sessionLive || cols < 1 || rows < 1) return;
      resizeSession(chatId, cols, rows).catch(() => {});
    },
    write: (data: string) => {
      if (!sessionLive) return;
      writeSession(chatId, data).catch(() => {});
    }
  };
}
