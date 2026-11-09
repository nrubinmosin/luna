import { call } from './core';
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

/** Luna's copy of a chat's handoff `n`, as text. */
export const handoffFile = (chatId: string, n: number) => call<string>('handoff_file', { chatId, n });

/** The conversation handoff `n` left, read-only, from `since` on. */
export const handoffTranscript = (chatId: string, n: number, since: number | null) =>
  call<HelperTranscriptDto>('handoff_transcript', { chatId, n, since });
