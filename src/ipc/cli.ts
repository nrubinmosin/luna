import { call, tauriAvailable } from './core';
import type { Provider } from '../shared/types';

/** Which versions a CLI's update pass installs (cli/choice.rs). */
export type CliChannel = 'knownGood' | 'latest';

export interface CliStatusDto {
  provider: Provider;
  phase: 'idle' | 'checking' | 'downloading' | 'error';
  /** Version Luna's own copy is at; null until the first download lands. */
  version: string | null;
  /** The binary sessions spawn; empty while there is no managed copy to start. */
  path: string;
  /** The newest the CLI's own release channel offers, once a check has come back. */
  latest: string | null;
  channel: CliChannel;
  got: number;
  total: number | null;
  error: string | null;
  /** Why the last pass left the version where it was on purpose. */
  note: string | null;
  checkedAtMs: number | null;
}

/** One installed version and what it is to Luna. */
export interface CliVersionRowDto {
  /** Its folder under `versions/`, which "Use" names. */
  dir: string;
  version: string;
  current: boolean;
  previous: boolean;
  /** A process of Luna's runs from it now. */
  inUse: boolean;
  knownGood: boolean;
  /** Older than `minSafe`: the backend neither makes it current nor marks it good. */
  belowMinSafe: boolean;
  /** Why it was marked bad, if it was. */
  bad: string | null;
  /** How it was marked good on this machine, if it was. */
  good: string | null;
}

/** The watch over a new version's first sessions (cli/canary.rs). */
export interface CliCanaryDto {
  version: string;
  watched: number;
  /** Watched sessions past their first half minute with no failure. */
  settled: number;
  failures: string[];
  warned: boolean;
  dismissed: boolean;
}

export interface CliVersionsDto {
  provider: Provider | null;
  channel: CliChannel;
  /** Newest first. */
  versions: CliVersionRowDto[];
  /** This Luna's shipped list, installed or not. */
  knownGood: string[];
  minSafe: string;
  floor: string;
  /** The folder "Use" holds the version at, while it does. */
  pinned: string | null;
  bad: Record<string, string>;
  /** Marked good here and still allowed to run, installed or not. */
  good: Record<string, string>;
  canary: CliCanaryDto | null;
}

export const cliStatus = (provider: Provider) => call<CliStatusDto | null>('cli_status', { provider }, null);
/** Check the release channel now and install whatever it names. */
export const cliUpdateNow = (provider: Provider) => call<void>('cli_update_now', { provider });
export const cliVersions = (provider: Provider) => call<CliVersionsDto | null>('cli_versions', { provider }, null);
/** The channels, "Use", the marks by hand and "Keep it" left the core with
 *  ADR-0030 (it rolls back on its own); this window goes with 0.20, so its
 *  buttons are only refused. */
const removed = (): Promise<void> => Promise.reject(new Error('no longer part of Luna (ADR-0030)'));
export const cliUseVersion: (provider: Provider, dir: string) => Promise<void> = removed;
export const cliSetChannel: (provider: Provider, channel: CliChannel) => Promise<void> = removed;
export const cliMarkBad: (provider: Provider, version: string, why?: string) => Promise<void> = removed;
export const cliMarkGood: (provider: Provider, version: string) => Promise<void> = removed;
export const cliClearMark: (provider: Provider, version: string) => Promise<void> = removed;
export const cliCanaryDismiss: (provider: Provider) => Promise<void> = removed;

export const pickFolder = async (): Promise<string | null> => {
  if (!tauriAvailable) return null;
  const { open } = await import('@tauri-apps/plugin-dialog');
  const res = await open({ directory: true, multiple: false });
  return typeof res === 'string' ? res : null;
};
