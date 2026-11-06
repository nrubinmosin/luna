import { useEffect, useState } from 'react';
import { recommendedSettings, type RecommendationDto } from '../../ipc/commands';
import { accountKey, type Account } from '../../shared/types';

/** "k/n": how many of an account's recommended settings its file has, or nothing with none to offer. */
export const recommendedCount = (list?: RecommendationDto[]) =>
  list?.length ? `${list.filter(r => r.applied).length}/${list.length}` : '';

/**
 * Each account's recommended settings, by key: what a button promises before
 * it is opened. Read again whenever `paused` goes off, so a dialog that can
 * change the file passes it while open.
 */
export function useRecommended(accounts: Account[], paused: boolean) {
  const [lists, setLists] = useState<Record<string, RecommendationDto[]>>({});
  const accountPaths = accounts.map(a => `${a.provider}|${a.path}`).join('\n');
  useEffect(() => {
    if (paused) return;
    let stale = false;
    void Promise.all(
      accounts.map(a =>
        recommendedSettings(a.provider, a.path)
          .then(list => [accountKey(a.provider, a.name), list] as const)
          .catch(() => [accountKey(a.provider, a.name), []] as const)
      )
    ).then(pairs => !stale && setLists(Object.fromEntries(pairs)));
    return () => {
      stale = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [accountPaths, paused]);
  return lists;
}
