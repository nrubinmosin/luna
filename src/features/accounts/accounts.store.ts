import { create } from 'zustand';
import type { Account, LimitSet } from '../../shared/types';
import { fmtReset } from '../../shared/lib/format';
import * as ipc from '../../ipc/commands';

// Folders on disk are the source of truth for the account list; limits come
// from the OAuth usage endpoint per account (zero token cost) and refresh on
// a timer.
const emptyLimits: LimitSet = { h5: 0, week: 0, fable: 0 };
const emptyResets = { h5: '—', week: '—', fable: '—' };

interface AccountsState {
  accounts: Account[];
  /** False until list_accounts has answered once. Sessions must not spawn
   *  before that: an unresolved account path means no CLAUDE_CONFIG_DIR. */
  loaded: boolean;
  adding: boolean;
  error: string | null;
  loginFor: Account | null;
  refresh: () => Promise<void>;
  add: (name: string) => Promise<void>;
  remove: (name: string) => Promise<void>;
  setAdding: (v: boolean) => void;
  setError: (e: string | null) => void;
  setLoginFor: (a: Account | null) => void;
}

const toAccount = (a: ipc.AccountInfo): Account => ({
  name: a.name,
  path: a.path,
  plan: '—',
  limits: emptyLimits,
  resets: emptyResets,
  sync: 'loading'
});

let retryTimer: ReturnType<typeof setTimeout> | undefined;
// Consecutive unhappy rounds. The previous fixed 8s retry turned a single 429
// into a loop that kept the usage endpoint throttled indefinitely.
let backoffStep = 0;
const BACKOFF_S = [30, 60, 120, 300, 600, 900];

const scheduleRetry = (afterS: number, run: () => void) => {
  clearTimeout(retryTimer);
  // Jitter keeps several accounts from hitting the endpoint in lockstep.
  const ms = afterS * 1000 + Math.random() * 3000;
  retryTimer = setTimeout(run, ms);
};

export const useAccounts = create<AccountsState>()((set, get) => ({
  accounts: [],
  loaded: false,
  adding: false,
  error: null,
  loginFor: null,

  refresh: async () => {
    const list = await ipc.listAccounts();
    // Show the accounts immediately, then fill limits in as they arrive.
    set(s => ({
      accounts: list.map(a => s.accounts.find(x => x.path === a.path) ?? toAccount(a)),
      loaded: true
    }));

    let serverWait = 0;
    const patch = (path: string, next: Partial<Account>) =>
      set(s => ({ accounts: s.accounts.map(x => (x.path === path ? { ...x, ...next } : x)) }));

    await Promise.allSettled(
      list.map(async a => {
        const lim = await ipc.accountLimits(a.path).catch(() => null);
        if (!lim) {
          patch(a.path, { sync: 'error' });
          return;
        }
        // Expired token: the CLI renews it as soon as a session runs, so keep
        // the last known numbers on screen instead of flashing zeros.
        if (lim.stale) {
          patch(a.path, { sync: 'stale' });
          return;
        }
        // Throttled: the numbers in this response are empty, not zero. Hold on
        // to whatever we last knew and wait as long as the server asked.
        if (lim.rateLimited != null) {
          patch(a.path, { sync: 'throttled' });
          serverWait = Math.max(serverWait, lim.rateLimited);
          return;
        }
        patch(a.path, {
          sync: 'ready',
          plan: lim.plan ?? '—',
          limits: { h5: lim.h5, week: lim.week, fable: lim.model },
          resets: {
            h5: fmtReset(lim.resetH5),
            week: fmtReset(lim.resetWeek),
            fable: fmtReset(lim.resetModel)
          }
        });
      })
    );

    const unhappy = get().accounts.filter(a => a.sync !== 'ready');
    if (unhappy.length === 0) {
      backoffStep = 0;
      return;
    }
    // Back off on every unhappy round, and never sooner than the server asked.
    const step = BACKOFF_S[Math.min(backoffStep, BACKOFF_S.length - 1)];
    backoffStep += 1;
    scheduleRetry(Math.max(step, serverWait), () => void get().refresh());
  },

  add: async name => {
    const trimmed = name.trim();
    if (!trimmed) return;
    try {
      const created = await ipc.createAccount(trimmed);
      set({ adding: false, error: null });
      await get().refresh();
      // Open a first-login terminal so credentials land in the new folder.
      if (created) set({ loginFor: get().accounts.find(a => a.name === created.name) ?? null });
    } catch (e) {
      set({ error: String(e) });
    }
  },

  remove: async name => {
    try {
      await ipc.deleteAccount(name);
      await get().refresh();
    } catch (e) {
      set({ error: String(e) });
    }
  },

  setAdding: v => set({ adding: v, error: null }),
  setError: e => set({ error: e }),
  setLoginFor: a => set({ loginFor: a })
}));
