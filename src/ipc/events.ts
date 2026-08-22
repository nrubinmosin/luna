import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type { CliStatusDto } from './commands';

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
