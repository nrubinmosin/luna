import type { ChatStatus } from '../types';

export const STATUS: Record<ChatStatus, { label: string; color: string; anim: string }> = {
  working: { label: 'working', color: 'oklch(.68 .19 145)', anim: 'pulse 1.8s ease-in-out infinite' },
  waiting: { label: 'waiting', color: 'oklch(.76 .17 72)', anim: 'none' },
  background: { label: 'background', color: 'oklch(.66 .12 235)', anim: 'pulse 3.6s ease-in-out infinite' },
  resting: { label: 'idle', color: 'var(--faint)', anim: 'none' }
};
