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
  open: boolean;
  adding: boolean;
  error: string | null;
  loginFor: Account | null;
  refresh: () => Promise<void>;
  add: (name: string) => Promise<void>;
  remove: (name: string) => Promise<void>;
  toggleOpen: () => void;
  setAdding: (v: boolean) => void;
  setError: (e: string | null) => void;
  setLoginFor: (a: Account | null) => void;
}

const toAccount = (a: ipc.AccountInfo): Account => ({
  name: a.name,
  path: a.path,
  plan: '—',
  limits: emptyLimits,
  resets: emptyResets
});

export const useAccounts = create<AccountsState>()((set, get) => ({
  accounts: [],
  open: false,
  adding: false,
  error: null,
  loginFor: null,

  refresh: async () => {
    const list = await ipc.listAccounts();
    // Show the accounts immediately, then fill limits in as they arrive.
    set(s => ({
      accounts: list.map(a => s.accounts.find(x => x.path === a.path) ?? toAccount(a))
    }));
    await Promise.allSettled(
      list.map(async a => {
        const lim = await ipc.accountLimits(a.path).catch(() => null);
        if (!lim) return;
        set(s => ({
          accounts: s.accounts.map(x =>
            x.path === a.path
              ? {
                  ...x,
                  plan: lim.plan ?? x.plan,
                  limits: { h5: lim.h5, week: lim.week, fable: lim.model },
                  resets: {
                    h5: fmtReset(lim.resetH5),
                    week: fmtReset(lim.resetWeek),
                    fable: fmtReset(lim.resetModel)
                  }
                }
              : x
          )
        }));
      })
    );
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

  toggleOpen: () => set(s => ({ open: !s.open })),
  setAdding: v => set({ adding: v, error: null }),
  setError: e => set({ error: e }),
  setLoginFor: a => set({ loginFor: a })
}));
