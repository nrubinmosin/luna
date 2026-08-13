import { create } from 'zustand';
import { persist } from 'zustand/middleware';
import type { Chat, Folder } from '../../shared/types';
import { usePanes } from '../panes/panes.store';

interface ChatsState {
  folders: Folder[];
  active: string | null;
  addChat: (folderPath: string, chat: Chat) => void;
  deleteChat: (chatId: string) => void;
  toggleFolder: (folderId: string) => void;
  setActive: (chatId: string | null) => void;
  setStatus: (chatId: string, status: Chat['status']) => void;
  setName: (chatId: string, name: string) => void;
  setWorktreePath: (chatId: string, path: string) => void;
  setSessionId: (chatId: string, sessionId: string) => void;
  setContext: (chatId: string, context: number, tokens: number | null, window: number | null) => void;
  setArchived: (chatId: string, archived: boolean) => void;
  toggleMark: (chatId: string) => void;
  renameChat: (chatId: string, name: string) => void;
  findChat: (chatId: string | null) => Chat | null;
  folderOf: (chatId: string) => Folder | null;
}

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
          return { folders, active: chat.id };
        }),

      deleteChat: chatId => {
        // Panes are cleared here rather than by the caller: a chat can be
        // deleted while it is open, and a board left holding the id of a chat
        // that no longer exists is a pane with nothing to render.
        usePanes.getState().evictChat(chatId);
        set(s => ({
          folders: s.folders
            .map(f => ({ ...f, chats: f.chats.filter(c => c.id !== chatId) }))
            .filter(f => f.chats.length > 0),
          active: s.active === chatId ? null : s.active
        }));
      },

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

      setName: (chatId, name) =>
        set(s => ({
          folders: s.folders.map(f => ({
            ...f,
            chats: f.chats.map(c => (c.id === chatId && c.name !== name ? { ...c, name } : c))
          }))
        })),

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

      setArchived: (chatId, archived) => {
        // Out of the panes as well as out of the list: a hidden chat holding a
        // pane would leave a slab of terminal on screen with no row to close
        // it from.
        if (archived) usePanes.getState().evictChat(chatId);
        set(s => ({
          folders: s.folders.map(f => ({
            ...f,
            chats: f.chats.map(c => (c.id === chatId ? { ...c, archived } : c))
          })),
          // An archived chat is out of sight; leaving it selected would leave
          // the rest of the UI reporting on something nobody can see.
          active: archived && s.active === chatId ? null : s.active
        }));
      },

      toggleMark: chatId =>
        set(s => ({
          folders: s.folders.map(f => ({
            ...f,
            chats: f.chats.map(c => (c.id === chatId ? { ...c, marked: !c.marked } : c))
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
            chats: f.chats.map(c => (c.id === chatId ? { ...c, name, nameCustom: true } : c))
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
    { name: 'luna.chats' }
  )
);
