import { create } from 'zustand';
import { persist } from 'zustand/middleware';
import type { Chat, Folder, FolderStart, GroupId, NameSource } from '../../shared/types';
import { usePanes } from '../panes/panes.store';
import { forget } from '../panes/terminals';

interface ChatsState {
  /**
   * The folders a chat has ever been launched from, kept whether or not
   * anything is in them: this is the list the new-chat dialog offers, and it
   * would be useless if a folder vanished the moment its last chat was
   * deleted. Their `chats` span every group; the sidebar shows one group's.
   */
  folders: Folder[];
  active: string | null;
  addChat: (folderPath: string, chat: Chat) => void;
  deleteChat: (chatId: string) => void;
  /** Forgets a launch folder. Only offered while nothing is left in it. */
  removeFolder: (folderId: string) => void;
  /** Records a folder without creating anything in it, e.g. after Browse. */
  rememberFolder: (folderPath: string) => void;
  /** What a chat the user just made in a folder started with. */
  rememberStart: (folderPath: string, start: FolderStart) => void;
  toggleFolder: (folderId: string) => void;
  setActive: (chatId: string | null) => void;
  setStatus: (chatId: string, status: Chat['status']) => void;
  /** Unfold or fold the children an agent spawned under this chat. */
  setChildrenOpen: (chatId: string, open: boolean) => void;
  /** Names a chat from somewhere other than the user — a no-op where the
   *  name it has may not be replaced from there (see `mayRename`). */
  setName: (chatId: string, name: string, source: NameSource) => void;
  setWorktreePath: (chatId: string, path: string) => void;
  setSessionId: (chatId: string, sessionId: string) => void;
  /** The model a Codex session reports running (its `turn_context`). */
  setModelSeen: (chatId: string, model: string) => void;
  setContext: (chatId: string, context: number, tokens: number | null, window: number | null) => void;
  setColor: (chatId: string, color: string | null) => void;
  renameChat: (chatId: string, name: string) => void;
  findChat: (chatId: string | null) => Chat | null;
  folderOf: (chatId: string) => Folder | null;
}

/** Folders holding chats of one group, with only that group's chats in them.
 *  A folder with nothing in this group is not shown: it stays in the launch
 *  list, which is the new-chat dialog's business, not the sidebar's. */
export const foldersOfGroup = (folders: Folder[], group: GroupId): Folder[] =>
  folders
    .map(f => ({ ...f, chats: f.chats.filter(c => c.group === group) }))
    .filter(f => f.chats.length > 0);

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

/** The colours worn in one group, for dealing a new chat one of its own. */
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

/** The newest chat the user made in a folder, for folders that had chats
 *  before the folder kept its own record of them. */
const lastMade = (chats: Chat[]): FolderStart | undefined => {
  const c = chats.filter(c => !c.parentId).at(-1);
  return c && { worktree: c.worktree, tools: c.tools ?? false };
};

let seq = 0;
export const newId = (prefix: string) => `${prefix}${Date.now().toString(36)}${(seq++).toString(36)}`;

export const useChats = create<ChatsState>()(
  persist(
    (set, get) => ({
      folders: [],
      active: null,

      addChat: (folderPath, chat) =>
        set(s => {
          const existing = s.folders.find(f => f.path === folderPath);
          const folders = existing
            ? s.folders.map(f =>
                f.path === folderPath ? { ...f, open: true, chats: [...f.chats, chat] } : f
              )
            : [...s.folders, { id: newId('f'), path: folderPath, open: true, chats: [chat] }];
          // A child an agent spawned does not take the selection: the parent
          // is what the user is looking at.
          return { folders, active: chat.parentId ? s.active : chat.id };
        }),

      setChildrenOpen: (chatId, open) =>
        set(s => ({
          folders: s.folders.map(f => ({
            ...f,
            chats: f.chats.map(c => (c.id === chatId ? { ...c, childrenOpen: open } : c))
          }))
        })),

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

      removeFolder: folderId =>
        set(s => ({ folders: s.folders.filter(f => f.id !== folderId || f.chats.length > 0) })),

      rememberStart: (folderPath, start) =>
        set(s => ({ folders: s.folders.map(f => (f.path === folderPath ? { ...f, last: start } : f)) })),

      rememberFolder: folderPath =>
        set(s =>
          s.folders.some(f => f.path === folderPath)
            ? s
            : { folders: [...s.folders, { id: newId('f'), path: folderPath, open: true, chats: [] }] }
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
            chats: f.chats.map(c =>
              c.id === chatId && c.worktreePath !== path ? { ...c, worktreePath: path } : c
            )
          }))
        })),

      setSessionId: (chatId, sessionId) =>
        set(s => ({
          folders: s.folders.map(f => ({
            ...f,
            chats: f.chats.map(c =>
              c.id === chatId && c.sessionId !== sessionId ? { ...c, sessionId } : c
            )
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

      setColor: (chatId, color) =>
        set(s => ({
          folders: s.folders.map(f => ({
            ...f,
            chats: f.chats.map(c => (c.id === chatId ? { ...c, color } : c))
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

      folderOf: chatId => get().folders.find(f => f.chats.some(c => c.id === chatId)) ?? null
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
        return {
          ...current,
          ...p,
          // Group is what decides whether a chat is listed at all, so a stored
          // chat without one would be invisible in every group.
          folders: (p.folders ?? []).map(f => ({
            ...f,
            chats: (f.chats ?? []).map(c => named({ ...c, group: c.group ?? 0 })),
            last: f.last ?? lastMade(f.chats ?? [])
          }))
        };
      }
    }
  )
);
