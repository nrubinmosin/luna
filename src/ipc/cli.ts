import { call, tauriAvailable } from './core';
import type { Provider } from '../shared/types';

export interface CliStatusDto {
  provider: Provider;
  phase: 'idle' | 'checking' | 'downloading' | 'error';
  /** Version Luna's own copy is at; null until the first download lands. */
  version: string | null;
  /** The binary sessions spawn — the bare name on PATH while there is no managed copy. */
  path: string;
  latest: string | null;
  got: number;
  total: number | null;
  error: string | null;
  checkedAtMs: number | null;
}

export const cliStatus = (provider: Provider) => call<CliStatusDto | null>('cli_status', { provider }, null);
/** Check the release channel now and install whatever is newer. */
export const cliUpdateNow = (provider: Provider) => call<void>('cli_update_now', { provider });

export const pickFolder = async (): Promise<string | null> => {
  if (!tauriAvailable) return null;
  const { open } = await import('@tauri-apps/plugin-dialog');
  const res = await open({ directory: true, multiple: false });
  return typeof res === 'string' ? res : null;
};
