import { create } from 'zustand';
import { persist } from 'zustand/middleware';
import type {
  Chat,
  Folder,
  FolderStart,
  GroupId,
  HandoffAsk,
  HandoffAsked,
  HandoffMark,
  NameSource
} from '../../shared/types';
import { useNewChat } from '../new-chat/newchat.store';
import { usePanes } from '../panes/panes.store';
import { forget } from '../panes/terminals';

interface ChatsState {
  /**
   * The folders a chat has ever been launched from, kept whether or not
   * anything is in them: the `picked` ones are the list the new-chat dialog
   * offers, and it would be useless if a folder vanished the moment its last
   * chat was deleted. Their `chats` span every group; the sidebar shows one
   * group's.
   */
  folders: Folder[];
  active: string | null;
  /** Chats that had the wait guard without asking, before it became each
   *  chat's choice (0.11.0), until the notice saying so is dismissed. */
  guardNotice: string[];
  dismissGuardNotice: () => void;
  /** The chat's wait guard, from its CLI's next start. */
  setGuard: (chatId: string, on: boolean) => void;
  /** The chat's auto-handoff and its threshold (handoff.ts `applyAutoHandoff`
   *  tells the core). */
  setAutoHandoff: (chatId: string, on: boolean, at: number) => void;
  /** Its latest handoff, as the core said. */
  setHandoffMark: (chatId: string, mark: HandoffMark | null) => void;
  /** A handoff it was asked for and has not made, as the core said. */
  setHandoffAsked: (chatId: string, asked: HandoffAsked | null) => void;
  setHandoffAsk: (chatId: string, ask: HandoffAsk | null) => void;
  /** Where a move to another account left the chat: gone on as `movedTo`,
   *  or opened from `movedFrom`. */
  setMoved: (chatId: string, moved: { movedTo?: string; movedFrom?: string }) => void;
  /** `byPerson`: launched by the user from the new-chat dialog, which makes
   *  the folder one the dialog offers; a helper opened as a chat is not. */
  addChat: (folderPath: string, chat: Chat, byPerson: boolean) => void;
  deleteChat: (chatId: string) => void;
  /** Forgets a launch folder. Only offered while nothing is left in it. */
  removeFolder: (folderId: string) => void;
  /** Records a folder without creating anything in it, e.g. after Browse. */
  rememberFolder: (folderPath: string) => void;
  /** Keeps how the new-chat checkboxes start in this folder; undefined
   *  hands it back to the defaults in Settings. */
  setStart: (folderPath: string, start: FolderStart | undefined) => void;
  toggleFolder: (folderId: string) => void;
  setActive: (chatId: string | null) => void;
  setStatus: (chatId: string, status: Chat['status']) => void;
  /** Names a chat from somewhere other than the user — a no-op where the
   *  name it has may not be replaced from there (see `mayRename`). */
  setName: (chatId: string, name: string, source: NameSource) => void;
  setWorktreePath: (chatId: string, path: string) => void;
  setSessionId: (chatId: string, sessionId: string) => void;
  /** The model a Codex session reports running (its `turn_context`). */
  setModelSeen: (chatId: string, model: string) => void;
  setContext: (chatId: string, context: number, tokens: number | null, window: number | null) => void;
  setColor: (chatId: string, color: string | null) => void;
  /** The browser the chat's links open in; null forgets it. */
  setBrowser: (chatId: string, browser: string | null) => void;
  renameChat: (chatId: string, name: string) => void;
  /** An account was renamed: its chats follow it. */
  renameAccount: (provider: Chat['provider'], from: string, to: string) => void;
  findChat: (chatId: string | null) => Chat | null;
  /**
   * Removes group `g` when no chat lives in it, renumbering the chats of the
   * groups after it, and says whether it did. Moving chats out of a group is
   * the user's job, not a side effect of removing it.
   */
  removeGroup: (g: GroupId) => boolean;
  folderOf: (chatId: string) => Folder | null;
}

/** Folders holding chats of one group, with only that group's chats in them.
 *  A folder with nothing in this group is not shown: it stays in the launch
 *  list, which is the new-chat dialog's business, not the sidebar's. */
export const foldersOfGroup = (folders: Folder[], group: GroupId): Folder[] =>
  folders.map(f => ({ ...f, chats: f.chats.filter(c => c.group === group) })).filter(f => f.chats.length > 0);

/**
 * One group's chats in the order the sidebar lists them, folded-away folders
 * skipped. This is the run the numbers on the rows count through and the one
 * Ctrl+<digit> resolves against, so the two cannot disagree about which chat
 * is the third one.
 */
export const numberedChats = (folders: Folder[], group: GroupId): Chat[] =>
  foldersOfGroup(folders, group)
    .filter(f => f.open)
    .flatMap(f => f.chats);

/** Every chat, whatever group it belongs to — for the session watcher and for
 *  working out which running sessions nothing claims. */
export const allChats = (folders: Folder[]): Chat[] => folders.flatMap(f => f.chats);

/** The colors worn in one group, for dealing a new chat one of its own. */
export const wornColors = (folders: Folder[], group: GroupId): Array<string | null | undefined> =>
  allChats(folders)
    .filter(c => c.group === group)
    .map(c => c.color);

/**
 * Whether a name from `next` may replace one from `current`. A chat is named
 * once and then left alone: the title bar used to swap between the CLI's
 * title, the line just typed and a plan's handle with every message. Only the
 * user's own word moves it on — a rename in Luna, which is final, or one in
 * the CLI.
 */
export const mayRename = (current: NameSource | undefined, next: NameSource): boolean => {
  switch (next) {
    case 'user':
      return true;
    case 'rename':
      return current !== 'user';
    case 'cli':
      return current === undefined || current === 'prompt';
    case 'prompt':
      return current === undefined || current === 'prompt';
  }
};

/** A stored chat without a `nameSource`: from before there was one, when a
 *  flag said only whether the user had named it, or still on its placeholder.
 *  One with a session has been called something by now, and the CLI's title
 *  may still replace it — that is what puts right the ones a plan's handle got
 *  to. */
const named = (c: Chat & { nameCustom?: boolean }): Chat => {
  if (c.nameSource) return c;
  const { nameCustom, ...rest } = c;
  return { ...rest, nameSource: nameCustom ? 'user' : rest.sessionId ? 'prompt' : undefined };
};

/** What a chat row an earlier Luna stored may still carry of the agent that
 *  spawned it as a visible helper; let go as it is read (`merge`). */
interface Spawned {
  parentId?: string | null;
  childrenOpen?: boolean;
}

let seq = 0;
export const newId = (prefix: string) => `${prefix}${Date.now().toString(36)}${(seq++).toString(36)}`;

/**
 * Chats stored before the wait guard was each chat's choice, which had it
 * without asking because they had Luna's tools, given `guard: false` as
 * every chat now starts: their ids come back, for the one notice that says
 * so (`guardNotice`). A row an earlier Luna nested under the chat whose
 * agent spawned it had the guard as a helper; it is the person's own chat
 * now (`merge`) and starts without it like the rest, so it is named too.
 */
export function decideGuard(folders: Folder[]): { folders: Folder[]; unguarded: string[] } {
  const unguarded: string[] = [];
  const decided = folders.map(f => ({
    ...f,
    chats: (f.chats ?? []).map(c => {
      if (c.guard !== undefined) return c;
      if (c.tools || (c as Chat & Spawned).parentId) unguarded.push(c.id);
      return { ...c, guard: false };
    })
  }));
  return { folders: decided, unguarded };
}

export const useChats = create<ChatsState>()(
  persist(
    (set, get) => ({
      folders: [],
      active: null,

      addChat: (folderPath, chat, byPerson) =>
        set(s => {
          // A chat already listed is not listed twice: a moved chat's row
          // may be asked for from two places at once (chats/moving.ts).
          if (s.folders.some(f => f.chats.some(c => c.id === chat.id))) return { active: chat.id };
          const existing = s.folders.find(f => f.path === folderPath);
          const folders = existing
            ? s.folders.map(f =>
                f.path === folderPath
                  ? { ...f, open: true, chats: [...f.chats, chat], picked: f.picked || byPerson }
                  : f
              )
            : [...s.folders, { id: newId('f'), path: folderPath, open: true, chats: [chat], picked: byPerson }];
          return { folders, active: chat.id };
        }),

      deleteChat: chatId => {
        // Panes are cleared here rather than by the caller: a chat can be
        // deleted while it is open, and a board left holding the id of a chat
        // that no longer exists is a pane with nothing to render.
        usePanes.getState().evictChat(chatId);
        // And its terminal, which would otherwise sit in the warm set holding
        // listeners for a session that is being killed as this runs.
        forget(chatId);
        set(s => ({
          // The folder stays behind on purpose, empty or not — it is a place
          // you launch chats from, and having to browse for it again after
          // clearing it out is the annoyance this list exists to remove.
          folders: s.folders.map(f => ({ ...f, chats: f.chats.filter(c => c.id !== chatId) })),
          active: s.active === chatId ? null : s.active
        }));
      },

      removeFolder: folderId => set(s => ({ folders: s.folders.filter(f => f.id !== folderId || f.chats.length > 0) })),

      setStart: (folderPath, start) =>
        set(s => ({ folders: s.folders.map(f => (f.path === folderPath ? { ...f, start } : f)) })),

      rememberFolder: folderPath =>
        set(s =>
          s.folders.some(f => f.path === folderPath)
            ? { folders: s.folders.map(f => (f.path === folderPath ? { ...f, picked: true } : f)) }
            : { folders: [...s.folders, { id: newId('f'), path: folderPath, open: true, chats: [], picked: true }] }
        ),

      toggleFolder: folderId =>
        set(s => ({
          folders: s.folders.map(f => (f.id === folderId ? { ...f, open: !f.open } : f))
        })),

      setActive: chatId => set({ active: chatId }),

      setStatus: (chatId, status) =>
        set(s => ({
          folders: s.folders.map(f => ({
            ...f,
            chats: f.chats.map(c => (c.id === chatId ? { ...c, status } : c))
          }))
        })),

      setName: (chatId, name, source) => {
        // The watcher offers every chat its title every few seconds; most of
        // those change nothing and should not cost a render.
        const c = get().findChat(chatId);
        if (!c || !mayRename(c.nameSource, source) || (c.name === name && c.nameSource === source)) return;
        set(s => ({
          folders: s.folders.map(f => ({
            ...f,
            chats: f.chats.map(x => (x.id === chatId ? { ...x, name, nameSource: source } : x))
          }))
        }));
      },

      setWorktreePath: (chatId, path) =>
        set(s => ({
          folders: s.folders.map(f => ({
            ...f,
            chats: f.chats.map(c => (c.id === chatId && c.worktreePath !== path ? { ...c, worktreePath: path } : c))
          }))
        })),

      setSessionId: (chatId, sessionId) =>
        set(s => ({
          folders: s.folders.map(f => ({
            ...f,
            chats: f.chats.map(c => (c.id === chatId && c.sessionId !== sessionId ? { ...c, sessionId } : c))
          }))
        })),

      setModelSeen: (chatId, model) =>
        set(s => ({
          folders: s.folders.map(f => ({
            ...f,
            chats: f.chats.map(c =>
              c.id === chatId && c.provider === 'codex' && c.modelSeen !== model ? { ...c, modelSeen: model } : c
            )
          }))
        })),

      guardNotice: [],
      dismissGuardNotice: () => set({ guardNotice: [] }),

      setGuard: (chatId, guard) =>
        set(s => ({
          folders: s.folders.map(f => ({
            ...f,
            chats: f.chats.map(c => (c.id === chatId ? { ...c, guard } : c))
          }))
        })),

      setAutoHandoff: (chatId, autoHandoff, handoffAt) =>
        set(s => ({
          folders: s.folders.map(f => ({
            ...f,
            chats: f.chats.map(c => (c.id === chatId ? { ...c, autoHandoff, handoffAt } : c))
          }))
        })),

      setHandoffMark: (chatId, mark) => {
        const c = get().findChat(chatId);
        if (!c || JSON.stringify(c.handoff ?? null) === JSON.stringify(mark)) return;
        set(s => ({
          folders: s.folders.map(f => ({
            ...f,
            chats: f.chats.map(x => (x.id === chatId ? { ...x, handoff: mark } : x))
          }))
        }));
      },

      setHandoffAsked: (chatId, asked) => {
        const c = get().findChat(chatId);
        if (!c || JSON.stringify(c.handoffAsked ?? null) === JSON.stringify(asked)) return;
        set(s => ({
          folders: s.folders.map(f => ({
            ...f,
            chats: f.chats.map(x => (x.id === chatId ? { ...x, handoffAsked: asked } : x))
          }))
        }));
      },

      setHandoffAsk: (chatId, ask) => {
        const c = get().findChat(chatId);
        if (!c || JSON.stringify(c.handoffAsk ?? null) === JSON.stringify(ask)) return;
        set(s => ({
          folders: s.folders.map(f => ({
            ...f,
            chats: f.chats.map(x => (x.id === chatId ? { ...x, handoffAsk: ask } : x))
          }))
        }));
      },

      setMoved: (chatId, moved) => {
        const c = get().findChat(chatId);
        if (!c || Object.entries(moved).every(([k, v]) => c[k as keyof typeof moved] === v)) return;
        set(s => ({
          folders: s.folders.map(f => ({
            ...f,
            chats: f.chats.map(x => (x.id === chatId ? { ...x, ...moved } : x))
          }))
        }));
      },

      setColor: (chatId, color) =>
        set(s => ({
          folders: s.folders.map(f => ({
            ...f,
            chats: f.chats.map(c => (c.id === chatId ? { ...c, color } : c))
          }))
        })),

      setBrowser: (chatId, browser) =>
        set(s => ({
          folders: s.folders.map(f => ({
            ...f,
            chats: f.chats.map(c => (c.id === chatId ? { ...c, browser } : c))
          }))
        })),

      setContext: (chatId, context, tokens, window) =>
        set(s => ({
          folders: s.folders.map(f => ({
            ...f,
            chats: f.chats.map(c =>
              c.id === chatId ? { ...c, context, contextTokens: tokens, contextWindow: window } : c
            )
          }))
        })),

      renameAccount: (provider, from, to) =>
        set(s => ({
          folders: s.folders.map(f => ({
            ...f,
            chats: f.chats.map(c => (c.provider === provider && c.account === from ? { ...c, account: to } : c))
          }))
        })),

      renameChat: (chatId, name) =>
        set(s => ({
          folders: s.folders.map(f => ({
            ...f,
            chats: f.chats.map(c => (c.id === chatId ? { ...c, name, nameSource: 'user' as const } : c))
          }))
        })),

      findChat: chatId => {
        if (!chatId) return null;
        for (const f of get().folders) {
          const c = f.chats.find(x => x.id === chatId);
          if (c) return c;
        }
        return null;
      },

      folderOf: chatId => get().folders.find(f => f.chats.some(c => c.id === chatId)) ?? null,

      removeGroup: g => {
        const panes = usePanes.getState();
        if (panes.groups.length <= 1 || allChats(get().folders).some(c => c.group === g)) return false;
        set(s => ({
          folders: s.folders.map(f => ({
            ...f,
            chats: f.chats.map(c => (c.group > g ? { ...c, group: c.group - 1 } : c))
          }))
        }));
        panes.dropGroup(g);
        return true;
      }
    }),
    {
      name: 'luna.chats',
      // Chats gained a provider and per-provider settings; what was stored
      // before has neither, and a row that cannot say which CLI it runs is
      // not worth carrying over.
      version: 2,
      migrate: (persisted, version) =>
        version < 2 ? { folders: [], active: null } : (persisted as Partial<ChatsState>),
      merge: (persisted, current) => {
        const p = (persisted ?? {}) as Partial<ChatsState>;
        const guarded = decideGuard(p.folders ?? []);
        return {
          ...current,
          ...p,
          guardNotice: [...new Set([...(p.guardNotice ?? []), ...guarded.unguarded])],
          // Group is what decides whether a chat is listed at all, so a stored
          // chat without one would be invisible in every group.
          // `last` was what the folder's newest chat started with, learned
          // rather than chosen; a folder now keeps only a start it was given.
          folders: guarded.folders.map(({ last: _learned, ...f }: Folder & { last?: unknown }) => {
            const stored = (f.chats ?? []) as Array<Chat & Spawned>;
            return {
              ...f,
              // A chat an earlier Luna nested under the chat whose agent spawned
              // it is the person's own now (agents/handover.rs): a top-level row.
              chats: stored.map(({ parentId: _p, childrenOpen: _o, ...c }) => named({ ...c, group: c.group ?? 0 })),
              // Stored before folders said who launched there. A top-level chat,
              // a start kept from the dialog or the dialog's last folder is a
              // person's doing; an empty folder cannot say, and is let go as an
              // agent's — Browse brings it back.
              picked:
                f.picked ?? (stored.some(c => !c.parentId) || !!f.start || f.path === useNewChat.getState().lastFolder)
            };
          })
        };
      }
    }
  )
);
