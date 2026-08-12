import { create } from 'zustand';
import { persist } from 'zustand/middleware';
import type { Chat, Folder } from '../../shared/types';

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
  setContext: (chatId: string, context: number) => void;
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

      deleteChat: chatId =>
        set(s => ({
          folders: s.folders
            .map(f => ({ ...f, chats: f.chats.filter(c => c.id !== chatId) }))
            .filter(f => f.chats.length > 0),
          active: s.active === chatId ? null : s.active
        })),

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

      setContext: (chatId, context) =>
        set(s => ({
          folders: s.folders.map(f => ({
            ...f,
            chats: f.chats.map(c => (c.id === chatId ? { ...c, context } : c))
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
    { name: 'llm-desktop.chats' }
  )
);
