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
  fetchedAt: null,
  sync: 'loading'
});

const sleep = (ms: number) => new Promise(r => setTimeout(r, ms));

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
// The usage endpoint 429s without a retry-after header, so the server never
// tells us how long to hold off. Sit out a few minutes after any 429: the
// CLI's own cache keeps the bars honest while sessions are running anyway.
const THROTTLED_S = 180;
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
    // Rebuilding the array every poll gave it a new identity and re-rendered
    // the whole footer on a timer, which is what the flicker was.
    const cur = get();
    const same =
      cur.loaded &&
      cur.accounts.length === list.length &&
      list.every((a, i) => cur.accounts[i]?.path === a.path);
    if (!same) {
      set(s => ({
        accounts: list.map(a => s.accounts.find(x => x.path === a.path) ?? toAccount(a)),
        loaded: true
      }));
    }

    let serverWait = 0;
    // Collected and applied in one write at the end: one store update per
    // round instead of one per account.
    const patches = new Map<string, Partial<Account>>();
    const patch = (path: string, next: Partial<Account>) => patches.set(path, next);

    for (const [i, a] of list.entries()) {
      // One account at a time, with daylight between them: fired together, the
      // requests reached the usage endpoint in the same instant and the second
      // account spent its life 429'd.
      if (i > 0) await sleep(2000 + Math.random() * 2000);

      const lim = await ipc.accountLimits(a.path).catch(() => null);
      if (!lim) {
        patch(a.path, { sync: 'error' });
        continue;
      }
      if (lim.rateLimited != null) serverWait = Math.max(serverWait, lim.rateLimited);

      // Identity always applies: it comes off disk and is right even when the
      // usage endpoint is unreachable.
      const base = {
        plan: lim.plan ?? '—',
        email: lim.email,
        signedIn: lim.signedIn
      };
      const sync = lim.rateLimited != null ? 'throttled' as const : lim.stale ? 'stale' as const : null;

      if (lim.haveUsage) {
        patch(a.path, {
          ...base,
          haveUsage: true,
          usageAge: ageLabel(lim.fetchedAtMs),
          fetchedAt: lim.fetchedAtMs,
          limits: { h5: lim.h5, week: lim.week, fable: lim.model },
          resets: {
            h5: fmtReset(lim.resetH5),
            week: fmtReset(lim.resetWeek),
            fable: fmtReset(lim.resetModel)
          },
          sync: sync ?? 'ready'
        });
        continue;
      }
      // A round that brought nothing (throttled, token mid-refresh, transient
      // error) keeps the last real numbers on the bars instead of wiping them
      // to dashes for a minute — that alternation is what read as blinking.
      // Only their age keeps ticking.
      const prev = get().accounts.find(x => x.path === a.path);
      patch(a.path, {
        ...base,
        usageAge: ageLabel(prev?.fetchedAt ?? null),
        sync: sync ?? 'error'
      });
    }

    // Apply everything at once, and skip the write entirely when nothing moved
    // — an idle poll of unchanged accounts should cost no render at all.
    const changed = (a: Account, p: Partial<Account>) =>
      (Object.keys(p) as (keyof Account)[]).some(k => {
        const before = a[k];
        const after = p[k];
        return typeof before === 'object' && before !== null
          ? JSON.stringify(before) !== JSON.stringify(after)
          : before !== after;
      });

    if (get().accounts.some(a => { const p = patches.get(a.path); return p && changed(a, p); })) {
      set(s => ({
        accounts: s.accounts.map(a => {
          const p = patches.get(a.path);
          return p && changed(a, p) ? { ...a, ...p } : a;
        })
      }));
    }

    // "Unhappy" means we have nothing to show — an account on cached numbers is
    // perfectly serviceable and should not drive the retry cadence.
    const unhappy = get().accounts.filter(a => a.sync !== 'ready' && !a.haveUsage);
    if (unhappy.length === 0) backoffStep = 0;

    if (!polling) return;
    // One scheduler for both cases. A separate fixed-interval poll alongside
    // this would defeat the backoff entirely — which is exactly what a 60s
    // interval in the app shell used to do to a rate-limited endpoint.
    // A 429 stretches even the healthy cadence: an account riding on kept
    // numbers is fine to look at, but polling again in 60s just earns the
    // next 429 — that loop is what kept the endpoint throttled for good.
    const throttled = get().accounts.some(a => a.sync === 'throttled');
    const wait =
      unhappy.length === 0
        ? Math.max(HEALTHY_S, serverWait, throttled ? THROTTLED_S : 0)
        : Math.max(BACKOFF_S[Math.min(backoffStep++, BACKOFF_S.length - 1)], serverWait, throttled ? THROTTLED_S : 0);
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
