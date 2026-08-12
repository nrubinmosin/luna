import { useEffect } from 'react';
import { sessionMeta } from '../../ipc/commands';
import { notifyWaiting } from '../../shared/lib/notify';
import type { Chat } from '../../shared/types';
import { useAccounts } from '../accounts/accounts.store';
import { useChats } from './chats.store';

const EVERY_MS = 4000;

// The CLI registry reports its own status vocabulary; fold it into the
// three-state design palette.
const mapStatus = (s: string | null): Chat['status'] => {
  if (!s) return 'resting';
  if (s === 'busy' || s === 'working') return 'working';
  if (/wait|input|block|attention|permission/.test(s)) return 'waiting';
  return 'resting';
};

/**
 * Keeps every chat's status, title and context in step with the CLI.
 *
 * This used to live inside the terminal, which meant it only ran for chats
 * that happened to have a mounted pane: close the pane, or switch to a layout
 * that doesn't show the chat, and its row in the sidebar froze on whatever it
 * last said — most visibly "working" long after the agent had finished.
 * Polling from here covers every chat, visible or not, and does it once rather
 * than once per pane.
 */
export function useSessionWatch() {
  useEffect(() => {
    let stopped = false;

    const tick = async () => {
      const { folders } = useChats.getState();
      const { accounts } = useAccounts.getState();
      const chats = folders.flatMap(f => f.chats);

      // Sequential on purpose: each read touches the filesystem, and a burst of
      // them across many chats is exactly the main-thread hiccup we just fixed.
      for (const chat of chats) {
        if (stopped) return;
        const accountPath = accounts.find(a => a.name === chat.account)?.path;
        if (!accountPath) continue;

        const store = useChats.getState();
        const fresh = store.findChat(chat.id);
        if (!fresh) continue;

        const meta = await sessionMeta(chat.id, accountPath).catch(() => null);
        if (!meta) {
          // No live session — the pty exited or was never started. Anything
          // other than resting would be a lie the sidebar keeps telling.
          if (fresh.status !== 'resting') store.setStatus(chat.id, 'resting');
          continue;
        }

        // The registry name is `derived` in practice — the cwd folder, which
        // for a worktree run is a random codename. Prefer the CLI's own title
        // when it ever produces one, else the session's opening prompt.
        const titled = meta.nameSource === 'auto' || meta.nameSource === 'user';
        const title = (titled && meta.name) || meta.firstPrompt;
        if (title && !fresh.nameCustom && title !== fresh.name) store.setName(chat.id, title);

        const next = mapStatus(meta.status);
        if (next === 'waiting' && fresh.status !== 'waiting') void notifyWaiting(fresh.name);
        if (next !== fresh.status) store.setStatus(chat.id, next);

        if (meta.context != null && meta.context !== fresh.context) {
          store.setContext(chat.id, meta.context, meta.contextTokens ?? null, meta.contextWindow ?? null);
        }
        // Remember where --worktree actually put the session, for cleanup on delete.
        if (meta.cwd && /[\\/]\.claude[\\/]worktrees[\\/]/i.test(meta.cwd) && meta.cwd !== fresh.worktreePath) {
          store.setWorktreePath(chat.id, meta.cwd);
        }
        // Remember the CLI session id so an app restart can --resume it.
        if (meta.sessionId && meta.sessionId !== fresh.sessionId) store.setSessionId(chat.id, meta.sessionId);
      }
    };

    // A round can outlast the interval when there are many chats, so chain
    // rather than schedule — no overlapping passes.
    let timer: ReturnType<typeof setTimeout>;
    const loop = async () => {
      await tick();
      if (!stopped) timer = setTimeout(() => void loop(), EVERY_MS);
    };
    void loop();

    return () => {
      stopped = true;
      clearTimeout(timer);
    };
  }, []);
}
