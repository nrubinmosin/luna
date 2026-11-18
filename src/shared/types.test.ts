import { describe, expect, it } from 'vitest';
import { PERM_CLI, PERM_HINTS, permFromSetting, type PermMode } from './types';

describe('Claude Code permission modes', () => {
  it('spell each of Luna’s modes as the CLI takes it, and read it back', () => {
    expect(PERM_CLI).toEqual({
      Ask: 'default',
      Edits: 'acceptEdits',
      'Plan only': 'plan',
      Auto: 'auto',
      Bypass: 'bypassPermissions'
    });
    for (const p of Object.keys(PERM_HINTS) as PermMode[]) expect(permFromSetting(PERM_CLI[p])).toBe(p);
  });

  it('read a mode Luna offers no chat in as none', () => {
    expect(permFromSetting('dontAsk')).toBeNull();
    expect(permFromSetting('Auto')).toBeNull();
  });
});
