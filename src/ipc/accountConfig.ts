import { call } from './core';
import type { Provider } from '../shared/types';

export interface ConfigFileDto {
  path: string;
  text: string;
  /** False when the account has no such file yet; `text` is a starting point. */
  exists: boolean;
}
export const readAccountConfig = (provider: Provider, accountPath: string) =>
  call<ConfigFileDto>('read_account_config', { provider, accountPath });
export const writeAccountConfig = (provider: Provider, accountPath: string, text: string) =>
  call<void>('write_account_config', { provider, accountPath, text });

export interface ConfigProblemDto {
  message: string;
  /** 1-based, when the parser says where. */
  line: number | null;
  column: number | null;
}
/** Where the text stops being a file the CLI can read; null when it reads. */
export const checkAccountConfig = (provider: Provider, text: string) =>
  call<ConfigProblemDto | null>('check_account_config', { provider, text }, null);
/** The text laid out the way the CLI writes it, comments and order kept. */
export const formatAccountConfig = (provider: Provider, text: string) =>
  call<string>('format_account_config', { provider, text });
