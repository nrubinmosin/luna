import { create } from 'zustand';
import { agentBlockedAccounts, setAgentAccount } from '../../ipc/commands';
import type { Provider } from '../../shared/types';

/**
 * Which accounts agents may spawn on: all of them, minus the ones switched
 * off in the account row. The list lives in Rust's settings.json (agents.rs
 * checks it at spawn); this store mirrors it for the row's toggle.
 */
interface AgentAccounts {
  blocked: string[];
  init: () => Promise<void>;
  allowed: (provider: Provider, name: string) => boolean;
  setAllowed: (provider: Provider, name: string, allowed: boolean) => Promise<void>;
}

const key = (provider: Provider, name: string) => `${provider}/${name}`;

export const useAgentAccounts = create<AgentAccounts>((set, get) => ({
  blocked: [],
  init: async () => set({ blocked: await agentBlockedAccounts() }),
  allowed: (provider, name) => !get().blocked.includes(key(provider, name)),
  setAllowed: async (provider, name, allowed) => set({ blocked: await setAgentAccount(provider, name, allowed) })
}));
