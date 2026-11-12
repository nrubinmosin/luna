import { call } from './core';
import type { HandoffAsk } from '../shared/types';
import type { HelperTranscriptDto } from './agents';

/** What setting auto-handoff on a chat calls for: its CLI started again on
 *  the same conversation, for the tool to reach it (agents/handoff). */
export interface AutoHandoffDto {
  resume: boolean;
}

/** Auto-handoff on a chat at `threshold` percent, or off (null): in the core
 *  at once, the threshold and the tool's refusal with it. */
export const setAutoHandoff = (id: string, threshold: number | null) =>
  call<AutoHandoffDto>('set_auto_handoff', { id, threshold }, { resume: false });

/** What asking a chat for a handoff calls for: its CLI started again on the
 *  same conversation, for the tool to reach it; the request is typed once it
 *  has (agents/handoff `handoff_now`). */
export interface HandoffNowDto {
  resume: boolean;
}

/** The pane's Hand off now: the person asks the chat for a handoff. Rejects
 *  with what stands in the way. */
export const handoffNow = (chatId: string) => call<HandoffNowDto>('handoff_now', { chatId }, { resume: false });

/** The ask for a handoff open on a chat, or null: the core's word whether
 *  its CLI runs or not (agents/handoff `handoff_ask`). */
export const handoffAsk = (chatId: string) => call<HandoffAsk | null>('handoff_ask', { chatId }, null);

/** Luna's copy of a chat's handoff `n`, as text. */
export const handoffFile = (chatId: string, n: number) => call<string>('handoff_file', { chatId, n });

/** The conversation handoff `n` left, read-only, from `since` on. */
export const handoffTranscript = (chatId: string, n: number, since: number | null) =>
  call<HelperTranscriptDto>('handoff_transcript', { chatId, n, since });
