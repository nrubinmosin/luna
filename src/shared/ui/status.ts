import type { ChatStatus } from '../types';

/** Each state its own color, and nothing that moves: a dot that pulsed kept
 *  the page drawing frames for as long as a chat worked. */
export const STATUS: Record<ChatStatus, { label: string; color: string }> = {
  working: { label: 'working', color: 'oklch(.68 .19 145)' },
  waiting: { label: 'waiting', color: 'oklch(.76 .17 72)' },
  background: { label: 'background', color: 'oklch(.66 .12 235)' },
  resting: { label: 'idle', color: 'var(--faint)' }
};
