import { call } from './core';

export interface CoreDto {
  pid: number;
  version: string;
  build: string;
  fingerprint: string;
  started: number;
  sessions: number;
  /** Why it starts no session: it runs as administrator. */
  refused?: string | null;
}

export interface CoreStateDto {
  /** `elevated`: no core that is not run as administrator; `error` says why. */
  state: 'starting' | 'up' | 'down' | 'mismatch' | 'elevated';
  core: CoreDto | null;
  error: string | null;
}

export const coreState = () => call<CoreStateDto>('core_state', {}, { state: 'down', core: null, error: null });
export const coreRestart = () => call<void>('core_restart');

/** Why Luna is about to quit, for the log (lifecycle.rs); null takes it back. */
export const noteExit = (reason: string | null) => call<void>('note_exit', { reason });
/** Asks every session to quit as a delete would; resolves with how many. */
export const stopAllSessions = () => call<number>('stop_all_sessions', {}, 0);
