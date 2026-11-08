import { call } from './core';

/** Why Luna adds something to a CLI's start (applied.rs `Class`). */
export type AppliedClass = 'runtime' | 'boundary' | 'userAction' | 'workaround';

/** One thing Luna applied: what, its class, the value as it went (secrets
 *  left out), and why. */
export interface Applied {
  key: string;
  class: AppliedClass;
  value: string;
  why: string;
}

/** A chat's "What Luna applied": its latest start (a helper's latest turn),
 *  and what Luna wrote into its account that concerns it. */
export interface AppliedView {
  launch: Applied[];
  account: Applied[];
}

export const appliedByLuna = (chatId: string) =>
  call<AppliedView>('applied_by_luna', { chatId }, { launch: [], account: [] });
