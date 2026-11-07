import { call } from './core';

export type Turn = 'busy' | 'waiting' | 'idle';
export type PowerAction = 'shutdown' | 'restart' | 'hibernate' | 'sleep' | 'log';

export interface SessionActivityDto {
  id: string;
  turn: Turn;
  /** Exe names of the children keeping it busy, when any. */
  procs: string[];
  outputFresh: boolean;
  busy: boolean;
  /** Waiting only on what an API error left — a usage limit: counted as done. */
  stalled: string | null;
}

export interface ActivitySummaryDto {
  busy: number;
  waiting: number;
  stalled: number;
  idleSinceMs: number | null;
  sessions: SessionActivityDto[];
}

export interface PowerStateDto {
  keepAwake: boolean;
  /** The awake request is in force right now. */
  holding: boolean;
  armed: { action: PowerAction; quietS: number; armedAtMs: number; drain: boolean } | null;
  countdownEndsAtMs: number | null;
  summary: ActivitySummaryDto;
}

export const IDLE_POWER: PowerStateDto = {
  keepAwake: true,
  holding: false,
  armed: null,
  countdownEndsAtMs: null,
  summary: { busy: 0, waiting: 0, stalled: 0, idleSinceMs: null, sessions: [] }
};

export const powerState = () => call<PowerStateDto>('power_state', {}, IDLE_POWER);
export const setKeepAwake = (on: boolean) => call<PowerStateDto>('set_keep_awake', { on }, IDLE_POWER);
export const armPowerOff = (action: PowerAction, quietS: number, drain: boolean) =>
  call<PowerStateDto>('arm_power_off', { action, quietS, drain }, IDLE_POWER);
export const disarmPowerOff = () => call<PowerStateDto>('disarm_power_off', {}, IDLE_POWER);

/** The tray icon's awake dot, with who holds the PC in its tooltip; null takes it off. */
export const trayAwake = (holders: string | null) => call<void>('tray_awake', { holders });
