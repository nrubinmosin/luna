/**
 * The settings a new chat will open on, for one provider: what the files say
 * (`resolved`, with `from` naming the file per field) and what the dialog
 * currently shows (`settings`), which is the same until a control is touched.
 * A touched control is the user's: re-resolving after a change of account or
 * folder must not quietly take it back.
 */
import { useEffect, useRef, useState } from 'react';

export type From<S> = Partial<Record<keyof S, string>>;

export interface Draft<S extends object> {
  settings: S;
  resolved: S;
  from: From<S>;
  pick: <K extends keyof S>(key: K, value: S[K]) => void;
  /** Sets a value that follows from another pick — unless the user has set
   *  this one by hand. Leaves it untouched, so it keeps following. */
  follow: <K extends keyof S>(key: K, value: S[K]) => void;
  /** Sets a value that must change whether or not it was set by hand (a
   *  level the newly picked model does not take). Whether it was touched
   *  stays as it was. */
  force: <K extends keyof S>(key: K, value: S[K]) => void;
  /** What a field resolves to once another changed (the effort a newly
   *  picked model starts on, which Claude Code keeps per model): what the
   *  origin line names and "back to …" returns to. The value shown is not
   *  touched: setting it is the caller's, in the same step. `source`
   *  undefined: no file said it. */
  rebase: <K extends keyof S>(key: K, value: S[K], source: string | undefined) => void;
  /** Whether the user has set this field by hand. */
  touched: (key: keyof S) => boolean;
  revert: (key: keyof S) => void;
}

export function useSettingsDraft<S extends object>(
  stock: S,
  resolve: (accountPath: string, folder: string) => Promise<{ values: S; from: From<S> }>,
  accountPath: string,
  folder: string
): Draft<S> {
  const [resolved, setResolved] = useState<S>(stock);
  const [from, setFrom] = useState<From<S>>({});
  const [settings, setSettings] = useState<S>(stock);
  // In a ref because the resolver should not re-run just because something
  // was touched.
  const touched = useRef<Partial<Record<keyof S, true>>>({});

  useEffect(() => {
    let stale = false;
    void resolve(accountPath, folder).then(({ values, from: sources }) => {
      if (stale) return;
      setResolved(values);
      setFrom(sources);
      setSettings(prev => {
        const next = { ...values };
        for (const key of Object.keys(values) as (keyof S)[]) {
          if (touched.current[key]) next[key] = prev[key];
        }
        return next;
      });
    });
    return () => {
      stale = true;
    };
    // `resolve` is a module-level function per provider, stable by construction.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [accountPath, folder]);

  return {
    settings,
    resolved,
    from,
    pick: (key, value) => {
      touched.current[key] = true;
      setSettings(s => ({ ...s, [key]: value }));
    },
    follow: (key, value) => {
      if (!touched.current[key]) setSettings(s => ({ ...s, [key]: value }));
    },
    force: (key, value) => setSettings(s => ({ ...s, [key]: value })),
    rebase: (key, value, source) => {
      setResolved(r => ({ ...r, [key]: value }));
      setFrom(f => {
        const next = { ...f };
        if (source === undefined) delete next[key];
        else next[key] = source;
        return next;
      });
    },
    touched: key => !!touched.current[key],
    revert: key => {
      delete touched.current[key];
      setSettings(s => ({ ...s, [key]: resolved[key] }));
    }
  };
}
