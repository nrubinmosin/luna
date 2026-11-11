/**
 * A chat moved to another account of its CLI, the window's part of it
 * (agents/handoff/moving.rs has the rest): once the person has sent the
 * preview, the old chat stopped where the move says so, then the new chat's
 * row made, which starts its CLI; the core types the one line into it once
 * its prompt shows. Each step is the core's to look at and record, and each
 * is taken again safely: a window that restarts in the middle finds the move
 * in `moveList` and goes on from where it stands. Nothing here ever sends a
 * preview: only Send does, and a preview Luna restarted on is given up.
 */
import { useEffect } from 'react';
import { killSession } from '../../ipc/sessions';
import { moveList, moveStep, type MoveLaunch, type MoveStatusDto } from '../../ipc/moving';
import { PERM_CLI, type Account, type Chat } from '../../shared/types';
import { findAccount, useAccounts } from '../accounts/accounts.store';
import { createChat } from '../new-chat/create';
import { forget } from '../panes/terminals';
import { claude } from '../providers';
import { useChats } from './chats.store';

/** How often moves under way are looked at. */
const MOVES_EVERY_MS = 3000;

/** The name a moved chat takes. */
export const movedName = (name: string) => (name.endsWith(' (moved)') ? name : `${name} (moved)`);

/**
 * The launch a moved chat starts with: the old chat's folder, group, color,
 * tools, guard and auto-handoff, its permissions as they were, and the model
 * and effort given (the old ones where the new account has them, else its
 * defaults: the dialog's to say).
 */
export function launchFor(
  chat: Chat,
  folder: string,
  to: Account,
  model: string | null,
  effort: string,
  stopOld: boolean
): MoveLaunch {
  const base = {
    provider: chat.provider,
    account: to.name,
    accountPath: to.path,
    folder,
    stopOld,
    name: movedName(chat.name),
    color: chat.color ?? null,
    group: chat.group,
    tools: chat.tools ?? false,
    guard: chat.guard ?? false,
    autoHandoff: (chat.tools ?? false) && (chat.autoHandoff ?? false),
    handoffAt: chat.handoffAt,
    model,
    effort
  };
  if (chat.provider === 'claude') {
    // A Claude Code worktree is gone into by its name from the project, as
    // every start of a worktree chat does (terminals.ts `startSession`).
    const worktreeName = chat.worktree ? claude.worktreeName(folder, chat.worktreePath) : null;
    return {
      ...base,
      perm: chat.perm,
      permissionMode: PERM_CLI[chat.perm],
      readOnly: chat.readOnly ?? false,
      agent: chat.agent ?? null,
      worktreeName,
      worktreePath: worktreeName ? (chat.worktreePath ?? null) : null
    };
  }
  return {
    ...base,
    approval: chat.approval,
    sandbox: chat.sandbox,
    hooksUnasked: chat.hooksUnasked ?? false,
    worktree: false
  };
}

/** The new chat's row, from the launch the person confirmed. */
async function makeRow(m: MoveStatusDto, l: MoveLaunch) {
  const account = findAccount(useAccounts.getState().accounts, l.provider, l.account);
  if (!account) throw new Error(`account ${l.account} is not there any more`);
  const common = {
    id: m.newId,
    folder: l.folder,
    account,
    // Never trusted silently: a folder the account does not trust yet shows
    // its CLI's own screen, which the core waits out.
    trust: false,
    tools: l.tools,
    guard: l.guard,
    autoHandoff: l.autoHandoff,
    handoffAt: l.handoffAt,
    name: l.name,
    color: l.color,
    group: l.group,
    movedFrom: m.chatId
  };
  if (l.provider === 'claude') {
    await createChat({
      ...common,
      provider: 'claude',
      // In the old chat's Claude Code worktree by name: the row keeps its
      // path, and its start asks for it with `--worktree <name>`.
      worktree: !!l.worktreeName,
      inWorktree: l.worktreeName ? (l.worktreePath ?? undefined) : undefined,
      readOnly: l.readOnly,
      agent: l.agent ?? null,
      settings: {
        model: l.model ?? '',
        effort: l.effort ?? '',
        perm: l.perm ?? 'Ask'
      }
    });
  } else {
    await createChat({
      ...common,
      provider: 'codex',
      worktree: false,
      hooksUnasked: l.hooksUnasked,
      settings: {
        model: l.model ?? null,
        effort: l.effort ?? '',
        approval: l.approval ?? 'on-request',
        sandbox: l.sandbox ?? 'read-only'
      }
    });
  }
}

/** Takes the window's steps of move `m` as far as they go now. */
export async function carryOut(m: MoveStatusDto): Promise<MoveStatusDto> {
  const chats = useChats.getState();
  let s = m;
  if (s.phase === 'confirmed' && s.stopOld) {
    await killSession(s.chatId);
    forget(s.chatId);
    chats.setStatus(s.chatId, 'resting');
    s = await moveStep(s.chatId, s.moveId, 'oldStopped');
  }
  if ((s.phase === 'confirmed' && !s.stopOld) || s.phase === 'oldStopped') {
    if (!chats.findChat(s.newId)) {
      if (!s.launch) throw new Error('the move has no launch settings');
      await makeRow(s, s.launch);
    }
    s = await moveStep(s.chatId, s.moveId, 'rowCreated');
  }
  if (s.phase === 'done') chats.setMoved(s.chatId, { movedTo: s.newId });
  return s;
}

/** Looks at the moves under way and takes the window's part of each: at the
 *  start (a move a restart left half way) and every few seconds while any is
 *  under way. */
export function useMoves() {
  useEffect(() => {
    let stopped = false;
    let timer: ReturnType<typeof setTimeout> | null = null;
    const busy = new Set<string>();
    const look = async () => {
      let pending = false;
      try {
        for (const m of await moveList()) {
          if (m.phase === 'done') useChats.getState().setMoved(m.chatId, { movedTo: m.newId });
          if (m.phase !== 'confirmed' && m.phase !== 'oldStopped') {
            pending ||= m.phase === 'rowCreated' || m.phase === 'prompting';
            continue;
          }
          pending = true;
          if (busy.has(m.moveId)) continue;
          busy.add(m.moveId);
          void carryOut(m)
            .catch(() => {})
            .finally(() => busy.delete(m.moveId));
        }
      } catch {
        // No core yet: looked at again.
        pending = true;
      }
      if (!stopped) timer = setTimeout(() => void look(), pending ? MOVES_EVERY_MS : MOVES_EVERY_MS * 10);
    };
    void look();
    return () => {
      stopped = true;
      if (timer) clearTimeout(timer);
    };
  }, []);
}
