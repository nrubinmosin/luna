import { useEffect } from 'react';
import { EFFORTS, MODELS } from '../../shared/types';
import type { Effort, ModelLabel } from '../../shared/types';
import { ackChatRequest, trustFolder, type NewChatRequest } from '../../ipc/commands';
import { onNewChatRequest } from '../../ipc/events';
import { logInfo, logWarn } from '../../shared/lib/log';
import { newId, useChats } from '../chats/chats.store';
import { usePanes } from '../panes/panes.store';
import { useAccounts } from '../accounts/accounts.store';

/** Case-insensitive match against the labels the UI already uses. */
const pick = <T extends string>(options: readonly T[], raw: string | null): T | null => {
  if (!raw) return null;
  const hit = options.find(o => o.toLowerCase() === raw.trim().toLowerCase());
  return hit ?? null;
};

/** Waits for the account list, which is read from disk after the UI mounts. */
const accountsReady = async (timeoutMs = 10_000) => {
  const deadline = Date.now() + timeoutMs;
  while (!useAccounts.getState().loaded && Date.now() < deadline) {
    await new Promise(r => setTimeout(r, 100));
  }
  return useAccounts.getState().loaded;
};

/**
 * Opens a chat asked for on the command line — `llm-desktop-cli --new-chat
 * --folder <path>`. Requests arrive as files in a watched directory and are
 * answered the same way, so the caller gets a real exit code. Nothing about
 * this feature sits in a model's context until someone runs the command.
 */
export function useCliChats() {
  useEffect(() => {
    // Everything below reports back to the blocked llm-desktop-cli process:
    // it is waiting on this answer, so a silent return would hang it until it
    // times out with a useless message.
    const fail = (id: string, why: string) => {
      logWarn('cli', why);
      void ackChatRequest(id, why);
    };

    const open = async (req: NewChatRequest) => {
      // The account list is read from disk asynchronously; a request that
      // arrives first waits rather than being told there are no accounts.
      if (!(await accountsReady())) {
        fail(req.id, 'accounts did not load in time');
        return;
      }
      const { accounts } = useAccounts.getState();
      if (!accounts.length) {
        fail(req.id, 'no Claude accounts are configured');
        return;
      }

      // An unknown account name is a mistake worth reporting, not a silent
      // fallback onto someone else's credentials.
      const account = req.account
        ? accounts.find(a => a.name === req.account)
        : accounts[0];
      if (!account) {
        fail(req.id, `unknown account "${req.account}"`);
        return;
      }

      // Same reason the dialog does it: the CLI cannot show its trust prompt
      // under --worktree, and this chat has no dialog to warn in.
      try {
        await trustFolder(account.path, req.folder);
      } catch (e) {
        fail(req.id, `could not mark ${req.folder} trusted: ${String(e)}`);
        return;
      }

      const { folders, addChat } = useChats.getState();
      const n = folders.reduce((a, f) => a + f.chats.length, 0) + 1;
      const id = newId('c');

      addChat(req.folder, {
        id,
        name: `chat ${n}`,
        status: 'resting',
        model: pick<ModelLabel>(MODELS, req.model) ?? 'Sonnet',
        effort: pick<Effort>(EFFORTS, req.effort) ?? 'medium',
        // Not settable from the command line: whatever can run the command
        // would otherwise be choosing this session's permissions.
        perm: 'Bypass',
        context: 0,
        account: account.name,
        worktree: req.worktree ?? true,
        pendingPrompt: req.prompt?.trim() || null
      });
      usePanes.getState().autoPlace(id);
      logInfo('cli', `opened chat ${id} in ${req.folder} on ${account.name}`);
      void ackChatRequest(req.id, null);
    };

    let unlisten: (() => void) | undefined;
    let stopped = false;

    void (async () => {
      const un = await onNewChatRequest(r => void open(r));
      if (stopped) un();
      else unlisten = un;
    })();

    return () => {
      stopped = true;
      unlisten?.();
    };
  }, []);
}
