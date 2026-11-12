import { render } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import * as ipc from '../../ipc/library';
import { LIBRARY } from '../../dev/demo/fixtures';
import { InstructionsGrid } from './InstructionsGrid';

vi.mock('../../ipc/library', async importOriginal => ({
  ...(await importOriginal<typeof import('../../ipc/library')>()),
  librarySetInstructions: vi.fn(() => Promise.resolve(null))
}));

describe('InstructionsGrid', () => {
  const act = vi.fn((p: Promise<unknown>) => p.then(() => true));
  const grid = () => render(<InstructionsGrid o={LIBRARY} busy={false} act={act} edit={vi.fn()} remove={vi.fn()} />);

  it('gives an account’s file by radio, and instructions for some roles by a box of their own', () => {
    grid();
    // `main` and `work` compete for an account's file: radios in one group per account.
    const mainCell = document.getElementById('lib-ins-main-claude/personal') as HTMLInputElement;
    expect(mainCell.type).toBe('radio');
    expect(mainCell.checked).toBe(true);
    // `leading`, for leads only, goes per session: a box, ticked where its reach takes the account.
    const lead = document.getElementById('lib-ins-leading-claude/personal') as HTMLInputElement;
    expect(lead.type).toBe('checkbox');
    expect(lead.checked).toBe(true);
    expect(lead.disabled).toBe(true);
    expect(lead.closest('td')?.querySelector('label')?.title).toMatch(/reach picker takes it back/);
    const codex = document.getElementById('lib-ins-leading-codex/personal') as HTMLInputElement;
    expect(codex.checked).toBe(false);
    expect(codex.closest('td')?.querySelector('label')?.title).toBe(
      'Give its sessions on personal leading as Luna starts them'
    );
  });

  it('adds an account to a per-session entry without touching the others', async () => {
    vi.mocked(ipc.librarySetInstructions).mockClear();
    grid();
    await userEvent.click(document.getElementById('lib-ins-leading-codex/personal')!);
    expect(ipc.librarySetInstructions).toHaveBeenCalledWith('leading', ['claude', 'codex/personal']);
  });
});
