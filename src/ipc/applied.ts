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

/** A system skill as the core resolves it now (library/system.rs
 *  `SystemSkillState`): Luna's own, the library's override, or an override
 *  that will not do and Luna's own served in its place. */
export interface SystemSkillState {
  name: string;
  state: 'luna' | 'library' | 'broken';
  /** Luna's version: whose own text it is, where it is Luna's. */
  version: string;
  /** Of what is served. */
  sha256: string;
  /** Where the override is, or would go. */
  path: string;
  /** Why the override is not used, when it is broken. */
  problem: string | null;
}

/** For a chat with auto-handoff on: the instructions it would be served to
 *  write its handoff by, and skills of the same name its CLI may list
 *  (agents/handoff `HandoffApplied`). */
export interface HandoffApplied {
  instructions: SystemSkillState;
  sameNamed: string[];
}

/** A chat's "What Luna applied": its latest start (a helper's latest turn),
 *  and what Luna wrote into its account that concerns it. */
export interface AppliedView {
  launch: Applied[];
  account: Applied[];
  /** Why `account` may be missing writes Luna made (its ledger would not
   *  read, or missed one); null when it is whole. */
  incomplete: string | null;
  /** Its handoff's instructions, for a chat with auto-handoff on. */
  handoff: HandoffApplied | null;
}

export const appliedByLuna = (chatId: string) =>
  call<AppliedView>('applied_by_luna', { chatId }, { launch: [], account: [], incomplete: null, handoff: null });
