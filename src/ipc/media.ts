import { call } from './core';

/** Copies a pasted/dropped file into the app's media store, returns its absolute path. */
export const saveMedia = (chatId: string, name: string, base64: string) =>
  call<string | null>('save_media', { chatId, name, data: base64 }, null);
