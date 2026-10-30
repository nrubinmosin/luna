import { create } from 'zustand';
import {
  browsers, setBrowsers, SYSTEM_BROWSER,
  type BrowserDto, type BrowserSettingsDto, type BrowsersDto, type LinkContext
} from '../../ipc/commands';
import { useChats } from '../chats/chats.store';
import { logWarn } from '../../shared/lib/log';

/**
 * The browsers a link can open in, as the Rust side finds and keeps them
 * (browsers.rs), and the Shift+click chooser. Loaded when the settings or
 * the chooser open: a browser installed meanwhile shows up then.
 */
interface BrowsersState {
  data: BrowsersDto | null;
  load: () => Promise<void>;
  save: (settings: BrowserSettingsDto) => Promise<void>;
}

export const useBrowsers = create<BrowsersState>(set => ({
  data: null,
  load: async () => set({ data: await browsers() }),
  save: async settings => {
    try {
      set({ data: await setBrowsers(settings) });
    } catch (e) {
      logWarn('browsers', String(e));
      throw e;
    }
  }
}));

/** Every browser there is to pick, the system's handler first. */
export const choices = (d: BrowsersDto | null): BrowserDto[] => [
  { id: SYSTEM_BROWSER, name: 'System default', command: d?.system ?? '' },
  ...(d?.found ?? []),
  ...(d?.settings.custom ?? [])
];

/** What a link clicked in a chat goes by: its own browser, its account, its folder. */
export function linkContext(chatId: string): LinkContext {
  const s = useChats.getState();
  const chat = s.findChat(chatId);
  return {
    browser: chat?.browser ?? null,
    account: chat ? `${chat.provider}/${chat.account}` : null,
    folder: s.folderOf(chatId)?.path ?? null
  };
}

interface ChooserState {
  at: { url: string; chatId: string; x: number; y: number } | null;
  show: (url: string, chatId: string, x: number, y: number) => void;
  hide: () => void;
}

export const useLinkChooser = create<ChooserState>(set => ({
  at: null,
  show: (url, chatId, x, y) => {
    set({ at: { url, chatId, x, y } });
    void useBrowsers.getState().load();
  },
  hide: () => set({ at: null })
}));
