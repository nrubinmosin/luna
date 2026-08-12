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
  /** Refreshes now and keeps refreshing, honouring the backoff between rounds. */
  startPolling: () => void;
  stopPolling: () => void;
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
  email: null,
  signedIn: false,
  haveUsage: false,
  limits: emptyLimits,
  resets: emptyResets,
  usageAge: null,
  sync: 'loading'
});

/** "just now" / "6m ago" / "2h ago" — how old the usage numbers are. */
const ageLabel = (fetchedAtMs: number | null): string | null => {
  if (!fetchedAtMs) return null;
  const m = Math.round((Date.now() - fetchedAtMs) / 60000);
  if (m < 1) return 'just now';
  if (m < 60) return `${m}m ago`;
  const h = Math.round(m / 60);
  return h < 48 ? `${h}h ago` : `${Math.round(h / 24)}d ago`;
};

let retryTimer: ReturnType<typeof setTimeout> | undefined;
// Consecutive unhappy rounds. The previous fixed 8s retry turned a single 429
// into a loop that kept the usage endpoint throttled indefinitely.
let backoffStep = 0;
const BACKOFF_S = [30, 60, 120, 300, 600, 900];

const HEALTHY_S = 60;
let polling = false;

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
        if (lim.rateLimited != null) serverWait = Math.max(serverWait, lim.rateLimited);

        // Identity always applies: it comes off disk and is right even when the
        // usage endpoint is unreachable.
        const base = {
          plan: lim.plan ?? '—',
          email: lim.email,
          signedIn: lim.signedIn,
          haveUsage: lim.haveUsage,
          usageAge: ageLabel(lim.fetchedAtMs)
        };

        // Numbers may be from the CLI's own cache; keep them even when this
        // round was throttled, and never overwrite real figures with zeros.
        if (lim.haveUsage) {
          patch(a.path, {
            ...base,
            limits: { h5: lim.h5, week: lim.week, fable: lim.model },
            resets: {
              h5: fmtReset(lim.resetH5),
              week: fmtReset(lim.resetWeek),
              fable: fmtReset(lim.resetModel)
            },
            sync: lim.rateLimited != null ? 'throttled' : lim.stale ? 'stale' : 'ready'
          });
          return;
        }
        patch(a.path, {
          ...base,
          sync: lim.rateLimited != null ? 'throttled' : lim.stale ? 'stale' : 'error'
        });
      })
    );

    // "Unhappy" means we have nothing to show — an account on cached numbers is
    // perfectly serviceable and should not drive the retry cadence.
    const unhappy = get().accounts.filter(a => a.sync !== 'ready' && !a.haveUsage);
    if (unhappy.length === 0) backoffStep = 0;

    if (!polling) return;
    // One scheduler for both cases. A separate fixed-interval poll alongside
    // this would defeat the backoff entirely — which is exactly what a 60s
    // interval in the app shell used to do to a rate-limited endpoint.
    const wait =
      unhappy.length === 0
        ? HEALTHY_S
        : Math.max(BACKOFF_S[Math.min(backoffStep++, BACKOFF_S.length - 1)], serverWait);
    scheduleRetry(wait, () => void get().refresh());
  },

  startPolling: () => {
    if (polling) return;
    polling = true;
    void get().refresh();
  },

  stopPolling: () => {
    polling = false;
    clearTimeout(retryTimer);
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
