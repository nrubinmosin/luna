import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type { AgentSpawnRequest, CliStatusDto, CoreStateDto, PowerStateDto } from './commands';

const tauriAvailable = '__TAURI_INTERNALS__' in window;

export interface PtyOutput {
  id: string;
  data: string;
}

export interface PtyExit {
  id: string;
  code: number | null;
}

export const onPtyOutput = (cb: (p: PtyOutput) => void): Promise<UnlistenFn> =>
  tauriAvailable ? listen<PtyOutput>('pty://output', e => cb(e.payload)) : Promise.resolve(() => {});

export const onCliStatus = (cb: (s: CliStatusDto) => void): Promise<UnlistenFn> =>
  tauriAvailable ? listen<CliStatusDto>('cli://status', e => cb(e.payload)) : Promise.resolve(() => {});

export const onPtyExit =(cb: (p: PtyExit) => void): Promise<UnlistenFn> =>
  tauriAvailable ? listen<PtyExit>('pty://exit', e => cb(e.payload)) : Promise.resolve(() => {});

export const onPowerState = (cb: (s: PowerStateDto) => void): Promise<UnlistenFn> =>
  tauriAvailable ? listen<PowerStateDto>('power://state', e => cb(e.payload)) : Promise.resolve(() => {});

export const onAgentSpawn = (cb: (r: AgentSpawnRequest) => void): Promise<UnlistenFn> =>
  tauriAvailable ? listen<AgentSpawnRequest>('agent://spawn', e => cb(e.payload)) : Promise.resolve(() => {});

/** The window's line to the core changed (corelink.rs). */
export const onCoreState = (cb: (s: CoreStateDto) => void): Promise<UnlistenFn> =>
  tauriAvailable ? listen<CoreStateDto>('core://state', e => cb(e.payload)) : Promise.resolve(() => {});

/** A desktop notification the Rust side wants shown (power.rs, agents.rs). */
export const onNotify = (cb: (n: { title: string; body: string }) => void): Promise<UnlistenFn> =>
  tauriAvailable ? listen<{ title: string; body: string }>('ui://notify', e => cb(e.payload)) : Promise.resolve(() => {});

/** A tray Quit with this many sessions running, for the window to confirm. */
export const onQuitAsked = (cb: (live: number) => void): Promise<UnlistenFn> =>
  tauriAvailable ? listen<number>('app://quit-asked', e => cb(e.payload)) : Promise.resolve(() => {});

export const onAgentDeleted = (cb: (p: { id: string }) => void): Promise<UnlistenFn> =>
  tauriAvailable ? listen<{ id: string }>('agent://deleted', e => cb(e.payload)) : Promise.resolve(() => {});
