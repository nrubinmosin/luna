import { call } from './core';

/** A browser a link can open in (browsers.rs): a name and a command line
 *  with `%1` where the link goes. */
export interface BrowserDto {
  id: string;
  name: string;
  command: string;
}

export interface BrowserSettingsDto {
  custom: BrowserDto[];
  /** Null: the system's handler. */
  default: string | null;
  /** By `provider/name`. */
  accounts: Record<string, string>;
  /** By project folder. */
  folders: Record<string, string>;
}

export interface BrowsersDto {
  /** The system handler's command line, where the system says. */
  system: string | null;
  found: BrowserDto[];
  settings: BrowserSettingsDto;
}

/** The id that stands for the system's handler. */
export const SYSTEM_BROWSER = 'system';

/** Where a link was clicked, for picking its browser: the chat's own choice,
 *  then its account's, its folder's, the default. */
export interface LinkContext {
  browser: string | null;
  account: string | null;
  folder: string | null;
}

export const browsers = () =>
  call<BrowsersDto>(
    'browsers',
    {},
    { system: null, found: [], settings: { custom: [], default: null, accounts: {}, folders: {} } }
  );
export const setBrowsers = (settings: BrowserSettingsDto) => call<BrowsersDto>('set_browsers', { settings });
/** A link from a terminal, in the browser its context comes to (lib.rs `open_link`). */
export const openLink = (url: string, ctx: LinkContext) => call<void>('open_link', { url, ctx });
