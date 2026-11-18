import type { ChatStatus } from '../types';

/** Each state its own color, and nothing that moves: a dot that pulsed kept
 *  the page drawing frames for as long as a chat worked. Background is a ring
 *  (`hollow`) where working is a dot, so the two differ in more than hue, as
 *  their pulses once made them. */
export const STATUS: Record<ChatStatus, { label: string; color: string; hollow?: boolean }> = {
  working: { label: 'working', color: 'oklch(.68 .19 145)' },
  waiting: { label: 'waiting', color: 'oklch(.76 .17 72)' },
  background: { label: 'background', color: 'oklch(.66 .12 235)', hollow: true },
  resting: { label: 'idle', color: 'var(--faint)' }
};
