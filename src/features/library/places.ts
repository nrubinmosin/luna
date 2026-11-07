import type { PlaceDto, Reach } from '../../ipc/library';
import { PROVIDER_LABEL, type Provider } from '../../shared/types';

// Which accounts a reach takes in, and what a copy's place says of it.

export const what = (p: PlaceDto) => p.skill ?? (p.instructions ? `${p.instructions} instructions` : 'Instructions');

export type AccountRef = { provider: Provider; name: string };
export const accountKey = (a: AccountRef) => `${a.provider}/${a.name}`;

/** How a reach takes in an account, as library.rs `closeness` ranks it: the closest wins it. */
export function how(reach: Reach, a: AccountRef): string | null {
  if (reach.includes(accountKey(a))) return 'named for it';
  if (reach.includes(a.provider)) return `as every ${PROVIDER_LABEL[a.provider]} account`;
  if (reach.includes('all')) return 'as every account';
  return null;
}

export function sayStatus(p: PlaceDto): string {
  switch (p.status) {
    case 'changed':
      return p.both
        ? 'changed there, and in the library too, since Luna wrote it'
        : 'changed there since Luna wrote it';
    case 'gone':
      return 'deleted there';
    case 'taken':
      return 'something Luna did not write is in its place';
    case 'failed':
      return p.problem ?? 'cannot be laid out';
    default:
      return p.status;
  }
}
