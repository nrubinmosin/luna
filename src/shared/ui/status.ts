import type { ChatStatus } from '../types';

export const STATUS: Record<ChatStatus, { label: string; color: string; anim: string }> = {
  working: { label: 'working', color: 'oklch(.64 .18 145)', anim: 'pulse 1.8s ease-in-out infinite' },
  waiting: { label: 'waiting', color: 'oklch(.79 .15 78)', anim: 'none' },
  resting: { label: 'idle', color: 'var(--faint)', anim: 'none' }
};
