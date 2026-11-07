import { call } from './core';
import type { Provider } from '../shared/types';

export interface RecommendationDto {
  /** Dotted path in the settings file, e.g. `tui` or `tui.show_tooltips`. */
  key: string;
  current: string | null;
  recommended: string;
  why: string;
  defaultOn: boolean;
  applied: boolean;
  /** The file it is kept in when that is not the account's settings file. */
  file: string | null;
}

export const recommendedSettings = (provider: Provider, accountPath: string) =>
  call<RecommendationDto[]>('recommended_settings', { provider, accountPath }, []);
/** Sets the `add` keys and takes the `remove` ones out of the file. */
export const applyRecommended = (provider: Provider, accountPath: string, add: string[], remove: string[]) =>
  call<void>('apply_recommended', { provider, accountPath, add, remove });

/** One thing a feature of Luna's relies on, as checks.rs finds it. */
export interface CheckDto {
  id: string;
  /** `provider/name`; null for what holds for the whole machine. */
  account: string | null;
  title: string;
  why: string;
  status: 'ok' | 'off' | 'blocked' | 'unknown';
  current: string | null;
  target: string | null;
  /** To show and have confirmed before `action`. */
  risk: string | null;
  note: string | null;
  action: string | null;
  /** Why `action` cannot be pressed just now. */
  refused: string | null;
  dismissed: boolean;
}

export const recommendations = () => call<CheckDto[]>('recommendations', {}, []);
/** Runs the check's fix and returns every check as it now stands. */
export const applyRecommendation = (id: string, account: string | null, confirmed: boolean) =>
  call<CheckDto[]>('apply_recommendation', { id, account, confirmed });
export const dismissRecommendation = (id: string, account: string | null, dismissed: boolean) =>
  call<CheckDto[]>('dismiss_recommendation', { id, account, dismissed });

export interface SyncStateDto {
  claude: string | null;
  codex: string | null;
  onStart: boolean;
}
export interface SyncReportDto {
  synced: string[];
  failed: string[];
}

const NO_SYNC: SyncStateDto = { claude: null, codex: null, onStart: false };
export const settingsSyncState = () => call<SyncStateDto>('settings_sync_state', {}, NO_SYNC);
export const setSettingsParent = (provider: Provider, parent: string | null) =>
  call<SyncStateDto>('set_settings_parent', { provider, parent }, NO_SYNC);
export const setSettingsSyncOnStart = (on: boolean) =>
  call<SyncStateDto>('set_settings_sync_on_start', { on }, NO_SYNC);
export const syncSettingsNow = (provider: Provider) => call<SyncReportDto>('sync_settings_now', { provider });
