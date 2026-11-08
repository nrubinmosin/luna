/**
 * The frontend's half of the agent layer (agents/). Every helper an agent
 * spawns runs in the background, in the core; the window lists them under the
 * chat whose agent runs them (background.store), shows one read-only, and opens
 * one as a chat of the person's own when asked (`openAsChat`). It also keeps the
 * core told of what only the window knows: the chats' names and groups, and the
 * accounts' usage.
 */
import { useEffect } from 'react';
import {
  accountUsage,
  backgroundHelpers,
  backgroundPromote,
  backgroundPromoted,
  chatNames,
  type BackgroundHelperDto,
  type PromotedDto
} from '../../ipc/agents';
import { onBackgroundHelpers } from '../../ipc/events';
import { logWarn } from '../../shared/lib/log';
import {
  accountKey,
  codexApprovalFromSetting,
  codexEffortFromSetting,
  codexSandboxFromSetting,
  effortFromSetting,
  modelFromSetting,
  permFromSetting,
  type ClaudeSettings,
  type CodexSettings
} from '../../shared/types';
import { useAccounts } from '../accounts/accounts.store';
import { allChats, useChats } from '../chats/chats.store';
import { useCore } from '../core/core.store';
import { createChat } from '../new-chat/create';
import { usePanes } from '../panes/panes.store';
import { claude, codex } from '../providers';
import { useBackground } from './background.store';

/**
 * Runs `again` whenever a core comes up: the registry the window keeps it
 * told of lives in the core, so one just started, or one that took over from
 * another, has none of it.
 */
const onCoreUp = (again: () => void) =>
  useCore.subscribe((s, prev) => {
    const pid = s.link.state === 'up' ? s.link.core?.pid : null;
    const was = prev.link.state === 'up' ? prev.link.core?.pid : null;
    if (pid != null && pid !== was) again();
  });

const samePath = (a: string, b: string) =>
  a.replace(/\//g, '\\').replace(/\\+$/, '').toLowerCase() === b.replace(/\//g, '\\').replace(/\\+$/, '').toLowerCase();

/** Helpers this page is opening as chats right now, which the settling below leaves to it. */
const opening = new Set<string>();

/**
 * Opens a background helper as a chat, handing it over to the person for good:
 * the agent that spawned it has no claim on it from then on, and hears so once
 * (agents/handover.rs). The core holds the helper, with no turn starting, until
 * the row is made, and lets it go only then: a row that could not be made leaves
 * the helper as it was, its agent's, never neither. The row is seated in a pane,
 * which resumes the conversation, only once the core has let go of it.
 */
export async function openAsChat(id: string): Promise<void> {
  // A second click while the first is on its way: one row, made once.
  if (opening.has(id)) return;
  opening.add(id);
  try {
    const p = await backgroundPromote(id);
    try {
      await promote(p);
    } catch (e) {
      const why = e instanceof Error ? e.message : String(e);
      await backgroundPromoted(id, why).catch(() => {});
      throw e;
    }
    if (await settleMade(id)) usePanes.getState().autoPlace(id);
  } finally {
    opening.delete(id);
  }
}

/** How long to wait before each try again to tell the core a row is made, in
 *  ms: a core restarting, or taken over by another, has it on a later try. Past
 *  the last, the next core to come up is told (`settleOpenings`), and one that
 *  never is gives the helper back to its agent (background/rows.rs
 *  `PROMOTE_TIMEOUT_MS`). */
export const SETTLE_RETRY_MS = [500, 1000, 2000, 4000, 8000, 15_000, 30_000, 60_000];

/**
 * Tells the core the row for helper `id` is made, trying again as long as
 * `SETTLE_RETRY_MS` says. True once the helper is the person's. A row made for
 * an opening the core has since given up is let go of here: the helper is its
 * agent's again, and the row would only run its conversation beside it.
 */
async function settleMade(id: string): Promise<boolean> {
  for (let i = 0; ; i++) {
    try {
      const handed = await backgroundPromoted(id, null);
      if (handed === false) {
        logWarn('agents', `helper ${id} went back to its agent before its opening was settled; its row is let go`);
        useChats.getState().deleteChat(id);
      }
      return handed !== false;
    } catch (e) {
      if (i >= SETTLE_RETRY_MS.length) {
        logWarn(
          'agents',
          `telling the core helper ${id} is opened failed: ${String(e)}; the next core to come up is told`
        );
        return false;
      }
      await new Promise(resolve => setTimeout(resolve, SETTLE_RETRY_MS[i]));
    }
  }
}

/**
 * Helpers a window that went away was opening as chats, settled by what this
 * one has: its row made, the core hands the helper over and the row is seated;
 * none, it goes on as a helper. A row with a helper's id is made by nothing but
 * an opening, so one whose opening the core gave up is let go of.
 */
function settleOpenings(rows: BackgroundHelperDto[]): void {
  for (const h of rows) {
    if (opening.has(h.id)) continue;
    const made = !!useChats.getState().findChat(h.id);
    if (!made && !h.promoting) continue;
    if (!made) {
      backgroundPromoted(h.id, 'the window that was opening it as a chat went away').catch(e =>
        logWarn('agents', `settling the opening of helper ${h.id} failed: ${String(e)}`)
      );
      continue;
    }
    opening.add(h.id);
    void settleMade(h.id)
      .then(handed => {
        if (handed) usePanes.getState().autoPlace(h.id);
      })
      .finally(() => opening.delete(h.id));
  }
}

/**
 * A background helper the person opened as a chat: a top-level row of their
 * own, keeping the helper's id, its conversation resumed when a pane first
 * shows it, once the core has let go of it.
 */
async function promote(p: PromotedDto): Promise<void> {
  const account = useAccounts
    .getState()
    .accounts.find(a => a.provider === p.provider && samePath(a.path, p.accountPath));
  if (!account) throw new Error(`no ${p.provider} account at ${p.accountPath}`);
  const s = p.settings as Record<string, string | null | undefined>;
  const word = (k: string) => (typeof s[k] === 'string' ? s[k] : '');
  const defaults = (await (p.provider === 'claude' ? claude : codex).settingsDefaults(account.path, p.folder)).values;
  const spec =
    p.provider === 'claude'
      ? {
          provider: 'claude' as const,
          settings: {
            model: modelFromSetting(word('model')) ?? (defaults as ClaudeSettings).model,
            effort: effortFromSetting(word('effort')) ?? (defaults as ClaudeSettings).effort,
            perm: permFromSetting(word('permissionMode')) ?? (defaults as ClaudeSettings).perm
          }
        }
      : {
          provider: 'codex' as const,
          settings: {
            model: word('model') || (defaults as CodexSettings).model,
            effort: codexEffortFromSetting(word('effort')) ?? (defaults as CodexSettings).effort,
            approval: codexApprovalFromSetting(word('approval')) ?? (defaults as CodexSettings).approval,
            sandbox: codexSandboxFromSetting(word('sandbox')) ?? (defaults as CodexSettings).sandbox
          }
        };
  const id = await createChat({
    ...spec,
    id: p.id,
    folder: p.folder,
    account,
    worktree: !!p.worktreePath,
    tools: p.tools,
    // As it ran in the background, which its folder decided.
    hooksUnasked: p.settings.hooksUnasked === true,
    // A helper always runs with the wait guard; the chat keeps it until the
    // person turns it off in its panel, rather than losing it on the handover.
    guard: true,
    name: p.name ?? undefined,
    inWorktree: p.worktreePath ?? undefined,
    fromHelper: true,
    readOnly: p.settings.readOnly === true,
    agent: word('agent') || null
  });
  if (p.sessionId) useChats.getState().setSessionId(id, p.sessionId);
}

/** Mounted once in App: keeps the helpers' list and the core's view of the chats. */
export function useAgentBridge() {
  useEffect(() => {
    let dead = false;
    const offs: Array<() => void> = [];
    const { setHelpers } = useBackground.getState();
    // Settled as this page comes up and whenever it finds the core again: what
    // a page gone meanwhile was opening it could not say.
    const helpersNow = (settle: boolean) =>
      void backgroundHelpers()
        .then(rows => {
          if (dead) return;
          setHelpers(rows);
          if (settle) settleOpenings(rows);
        })
        .catch(() => {});
    void onBackgroundHelpers(setHelpers).then(off => {
      if (dead) {
        off();
        return;
      }
      offs.push(off);
      helpersNow(true);
    });
    offs.push(onCoreUp(() => helpersNow(true)));
    return () => {
      dead = true;
      offs.forEach(off => off());
    };
  }, []);

  // The core knows sessions by id only; `luna_sessions` names them with the
  // rows' names and places them in their window groups, sent now and again
  // whenever one changes.
  useEffect(() => {
    let last = '';
    const push = () => {
      const chats = allChats(useChats.getState().folders);
      const names = Object.fromEntries(chats.map(c => [c.id, c.name]));
      const groups = Object.fromEntries(chats.map(c => [c.id, c.group]));
      const key = JSON.stringify([names, groups]);
      if (key === last) return;
      last = key;
      // Not sent (the core not up yet, or gone): not known there either, so
      // the next push must not take it for done.
      chatNames(names, groups).catch(() => {
        if (last === key) last = '';
      });
    };
    push();
    const offChats = useChats.subscribe(push);
    const offCore = onCoreUp(() => {
      last = '';
      push();
    });
    return () => {
      offChats();
      offCore();
    };
  }, []);

  // `luna_list` gives agents each account's usage as the Accounts window
  // shows it, so a helper goes where there is room; sent whenever it moves.
  useEffect(() => {
    let last = '';
    const push = () => {
      const usage = Object.fromEntries(
        useAccounts.getState().accounts.map(a => [
          accountKey(a.provider, a.name),
          {
            signedIn: a.signedIn,
            plan: a.plan,
            limits: a.haveUsage
              ? a.limits.map(l => ({ label: l.label, usedPct: Math.round(l.used * 100), resetsAt: l.resetAt }))
              : [],
            takenAt: a.haveUsage && a.fetchedAt ? new Date(a.fetchedAt).toISOString() : null
          }
        ])
      );
      const key = JSON.stringify(usage);
      if (key === last) return;
      last = key;
      accountUsage(usage).catch(() => {
        if (last === key) last = '';
      });
    };
    push();
    const offAccounts = useAccounts.subscribe(push);
    const offCore = onCoreUp(() => {
      last = '';
      push();
    });
    return () => {
      offAccounts();
      offCore();
    };
  }, []);
}
