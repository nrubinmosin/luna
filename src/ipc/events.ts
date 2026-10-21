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

export interface Notice {
  title: string;
  body: string;
  /** The chat it is about, opened by a click on it. */
  chatId: string | null;
}

/** A desktop notification the Rust side wants shown (power.rs, agents.rs). */
export const onNotify = (cb: (n: Notice) => void): Promise<UnlistenFn> =>
  tauriAvailable ? listen<Notice>('ui://notify', e => cb(e.payload)) : Promise.resolve(() => {});

/** A notification about a chat was clicked (lib.rs). */
export const onOpenChat = (cb: (p: { chatId: string }) => void): Promise<UnlistenFn> =>
  tauriAvailable ? listen<{ chatId: string }>('ui://open-chat', e => cb(e.payload)) : Promise.resolve(() => {});

export const onAgentDeleted = (cb: (p: { id: string }) => void): Promise<UnlistenFn> =>
  tauriAvailable ? listen<{ id: string }>('agent://deleted', e => cb(e.payload)) : Promise.resolve(() => {});
