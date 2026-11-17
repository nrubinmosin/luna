import { useEffect } from 'react';
import { savedTitle, sessionsMeta, type ChatLookDto } from '../../ipc/sessions';
import { notifyWaiting } from '../../shared/lib/notify';
import { PROVIDER_LABEL, type Chat } from '../../shared/types';
import { accountOfChat, useAccounts } from '../accounts/accounts.store';
import { ui } from '../providers';
import { useChats } from './chats.store';
import { useDrafts } from './drafts.store';
import { useMarked } from './marked.store';
import { useMoveHints } from './moveHints.store';

const EVERY_MS = 4000;

/** Chats whose saved title has been looked up this run — once is enough for
 *  a session that is not running, since nothing is writing to it. A chat
 *  deleted leaves it at the next tick. */
const titleLooked = new Set<string>();

// The CLI registry reports its own status vocabulary; fold it into the
// design palette. `background` is the core's own word, not a CLI's.
export const mapStatus = (s: string | null): Chat['status'] => {
  if (!s) return 'resting';
  if (s === 'background') return 'background';
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
 * than once per pane: one call a tick for all of them (`sessions_meta`), which
 * the core answers off what it has kept of each CLI's files while they are
 * unchanged.
 */
export function useSessionWatch() {
  useEffect(() => {
    let stopped = false;

    const tick = async () => {
      const { folders } = useChats.getState();
      const { accounts } = useAccounts.getState();
      const chats = folders.flatMap(f => f.chats);
      // The set lives as long as the page; a chat deleted leaves it here.
      const listed = new Set(chats.map(c => c.id));
      for (const id of titleLooked) if (!listed.has(id)) titleLooked.delete(id);

      const asked = chats.flatMap(chat => {
        const accountPath = accountOfChat(accounts, chat)?.path;
        return accountPath ? [{ chat, accountPath }] : [];
      });
      if (asked.length === 0) return;
      // A call that failed (no core) says no chat is running, as each chat's
      // own failed call used to; the ask for a handoff stays as it was.
      const looks: ChatLookDto[] = await sessionsMeta(
        asked.map(({ chat, accountPath }) => ({ id: chat.id, accountPath }))
      ).catch(() => []);
      const byId = new Map(looks.map(l => [l.id, l]));

      // Every chat's look goes in at once, with no wait between: a wait (a
      // saved title's) would leave the chats after it on what the core said
      // before it. The saved titles come after.
      const titles: { id: string; accountPath: string }[] = [];
      for (const { chat, accountPath } of asked) {
        if (stopped) return;
        const store = useChats.getState();
        const fresh = store.findChat(chat.id);
        if (!fresh) continue;

        const look = byId.get(chat.id);
        const meta = look?.meta ?? null;
        if (!meta) {
          // No live session — the pty exited or was never started. Anything
          // other than resting would be a lie the sidebar keeps telling.
          if (fresh.status !== 'resting') store.setStatus(chat.id, 'resting');
          useDrafts.getState().set(chat.id, null);
          useMarked.getState().set(chat.id, null);
          // The ask for a handoff is the core's, CLI or none: one that lapsed
          // while it was down must not keep the chip and the menu's refusal.
          if (look) store.setHandoffAsk(chat.id, look.handoffAsk ?? null);
          // A chat restored from an earlier run may still wear the opening
          // prompt it was named after; the CLI's title is on disk by now.
          if (fresh.sessionId && fresh.nameSource !== 'user' && !titleLooked.has(chat.id)) {
            titleLooked.add(chat.id);
            titles.push({ id: chat.id, accountPath });
          }
          continue;
        }

        // The CLI's own title — a rename, else the one it generated from the
        // conversation, the same it puts on a terminal tab — and the opening
        // prompt only until that exists. A first line alone rarely says what
        // the chat is about. setName keeps whatever the chat is already
        // settled on: the first generated title stays.
        if (meta.title) store.setName(chat.id, meta.title, meta.titleRenamed ? 'rename' : 'cli');
        else if (meta.firstPrompt) store.setName(chat.id, meta.firstPrompt, 'prompt');

        useDrafts.getState().set(chat.id, { draft: meta.draft, held: meta.held });
        if (meta.handoff) store.setHandoffMark(chat.id, meta.handoff);
        store.setHandoffAsked(chat.id, meta.handoffAsked ?? null);
        store.setHandoffAsk(chat.id, meta.handoffAsk ?? null);
        useMoveHints.getState().setLimit(chat.id, meta.limit ?? null);
        useMarked.getState().set(chat.id, meta.marked);

        const next = mapStatus(meta.status);
        if (next === 'waiting' && fresh.status !== 'waiting') {
          void notifyWaiting(fresh.id, fresh.name, PROVIDER_LABEL[fresh.provider]);
        }
        if (next !== fresh.status) store.setStatus(chat.id, next);

        if (meta.context != null && meta.context !== fresh.context) {
          store.setContext(chat.id, meta.context, meta.contextTokens ?? null, meta.contextWindow ?? null);
        }
        // Remember where the worktree actually is, for cleanup on delete and
        // to resume in: each CLI's `--worktree` only tells through the
        // session — Claude Code's registry, Codex's rollout.
        if (meta.cwd && ui(fresh.provider).worktreeRe.test(meta.cwd) && meta.cwd !== fresh.worktreePath) {
          store.setWorktreePath(chat.id, meta.cwd);
        }
        // Remember the CLI session id so an app restart can resume it.
        if (meta.sessionId && meta.sessionId !== fresh.sessionId) store.setSessionId(chat.id, meta.sessionId);
        // What Codex reports running, for a chat that left the model to it.
        if (fresh.provider === 'codex' && meta.model && meta.model !== fresh.modelSeen) {
          store.setModelSeen(chat.id, meta.model);
        }
      }

      for (const { id, accountPath } of titles) {
        if (stopped) return;
        const fresh = useChats.getState().findChat(id);
        if (!fresh?.sessionId || fresh.nameSource === 'user') continue;
        const saved = await savedTitle(fresh.provider, accountPath, fresh.sessionId).catch(() => null);
        if (saved) useChats.getState().setName(id, saved.title, saved.renamed ? 'rename' : 'cli');
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
