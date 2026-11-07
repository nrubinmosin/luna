import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import type { Account } from '../../shared/types';
import { AccountPicker } from './AccountPicker';

const account = (name: string): Account => ({
  provider: 'claude',
  name,
  path: `/accounts/${name}`,
  plan: 'max',
  email: null,
  signedIn: true,
  haveUsage: false,
  limits: [],
  usageAge: null,
  fetchedAt: null,
  sync: 'ready',
  refreshError: null
});

const [a, b, c] = [account('a'), account('b'), account('c')];

/** Opens the list on `value` and steps the highlight down `steps` rows. */
async function openAndStep(accounts: Account[], steps: number) {
  const onChange = vi.fn();
  const user = userEvent.setup();
  const view = render(<AccountPicker accounts={accounts} value="claude/a" onChange={onChange} />);
  await user.click(screen.getByRole('button'));
  for (let i = 0; i < steps; i++) await user.keyboard('{ArrowDown}');
  const rerender = (next: Account[]) =>
    view.rerender(<AccountPicker accounts={next} value="claude/a" onChange={onChange} />);
  return { user, onChange, rerender };
}

const highlighted = () => screen.getAllByRole('option').find(o => o.dataset.hot === 'yes');

describe('AccountPicker', () => {
  it('picks the highlighted account on Enter', async () => {
    const { user, onChange } = await openAndStep([a, b, c], 1);
    expect(highlighted()).toHaveTextContent('b');
    await user.keyboard('{Enter}');
    expect(onChange).toHaveBeenCalledExactlyOnceWith('claude/b');
    expect(screen.queryByRole('listbox')).not.toBeInTheDocument();
  });

  it('picks nothing on Enter once the highlighted account is gone', async () => {
    const { user, onChange, rerender } = await openAndStep([a, b, c], 1);
    rerender([a, c]);
    expect(highlighted()).toBeUndefined();
    await user.keyboard('{Enter}');
    expect(onChange).not.toHaveBeenCalled();
    expect(screen.queryByRole('listbox')).not.toBeInTheDocument();
  });

  it('keeps the highlight on its account through a reorder, and Enter picks it', async () => {
    const { user, onChange, rerender } = await openAndStep([a, b, c], 1);
    // b moves from the second row to the first; the second row is now a.
    rerender([b, a, c]);
    expect(highlighted()).toHaveTextContent('b');
    await user.keyboard('{Enter}');
    expect(onChange).toHaveBeenCalledExactlyOnceWith('claude/b');
  });
});
