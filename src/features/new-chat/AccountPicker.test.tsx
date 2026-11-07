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
  return { user, onChange, rerender, view };
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

  it('steps from nothing to the first row on ArrowDown once the highlighted account is gone', async () => {
    const { user, rerender } = await openAndStep([a, b, c], 1);
    rerender([a, c]);
    await user.keyboard('{ArrowDown}');
    expect(highlighted()).toHaveTextContent('a');
  });

  it('steps from nothing to the last row on ArrowUp once the highlighted account is gone', async () => {
    const { user, rerender } = await openAndStep([a, b, c], 1);
    rerender([a, c]);
    await user.keyboard('{ArrowUp}');
    expect(highlighted()).toHaveTextContent('c');
  });

  it('steps from the highlighted account’s new place after a reorder', async () => {
    const { user, rerender } = await openAndStep([a, b, c], 1);
    rerender([b, c, a]);
    await user.keyboard('{ArrowDown}');
    expect(highlighted()).toHaveTextContent('c');
    await user.keyboard('{ArrowUp}{ArrowUp}');
    expect(highlighted()).toHaveTextContent('a');
  });

  it('wraps round at either end', async () => {
    const { user } = await openAndStep([a, b, c], 0);
    await user.keyboard('{ArrowUp}');
    expect(highlighted()).toHaveTextContent('c');
    await user.keyboard('{ArrowDown}');
    expect(highlighted()).toHaveTextContent('a');
  });

  it('with no accounts, offers none and cannot be opened', async () => {
    const onChange = vi.fn();
    const user = userEvent.setup();
    render(<AccountPicker accounts={[]} value="" onChange={onChange} />);
    const face = screen.getByRole('button');
    expect(face).toBeDisabled();
    expect(face).toHaveTextContent('add an account first');
    await user.click(face);
    await user.keyboard('{ArrowDown}{Enter}');
    expect(screen.queryByRole('listbox')).not.toBeInTheDocument();
    expect(onChange).not.toHaveBeenCalled();
  });

  it('picks through the newest onChange when it is replaced while the list is open', async () => {
    const { user, onChange, view } = await openAndStep([a, b, c], 1);
    const next = vi.fn();
    view.rerender(<AccountPicker accounts={[a, b, c]} value="claude/a" onChange={next} />);
    await user.keyboard('{Enter}');
    expect(next).toHaveBeenCalledExactlyOnceWith('claude/b');
    expect(onChange).not.toHaveBeenCalled();
  });

  it('marks a value changed while open as selected, and keeps the keyboard’s highlight', async () => {
    const { user, onChange, view } = await openAndStep([a, b, c], 1);
    view.rerender(<AccountPicker accounts={[a, b, c]} value="claude/c" onChange={onChange} />);
    const selected = screen.getAllByRole('option').find(o => o.getAttribute('aria-selected') === 'true');
    expect(selected).toHaveTextContent('c');
    expect(highlighted()).toHaveTextContent('b');
    await user.keyboard('{Enter}');
    expect(onChange).toHaveBeenCalledExactlyOnceWith('claude/b');
  });

  it('starts the highlight on the current value each time it opens', async () => {
    const onChange = vi.fn();
    const user = userEvent.setup();
    const view = render(<AccountPicker accounts={[a, b, c]} value="claude/a" onChange={onChange} />);
    view.rerender(<AccountPicker accounts={[a, b, c]} value="claude/c" onChange={onChange} />);
    await user.click(screen.getByRole('button'));
    expect(highlighted()).toHaveTextContent('c');
  });

  it('steps the value with the arrows while closed, as a select does', async () => {
    const onChange = vi.fn();
    const user = userEvent.setup();
    render(<AccountPicker accounts={[a, b, c]} value="claude/a" onChange={onChange} />);
    screen.getByRole('button').focus();
    await user.keyboard('{ArrowUp}');
    expect(onChange).toHaveBeenCalledExactlyOnceWith('claude/c');
    expect(screen.queryByRole('listbox')).not.toBeInTheDocument();
  });

  it('closes on Escape without picking', async () => {
    const { user, onChange } = await openAndStep([a, b, c], 1);
    await user.keyboard('{Escape}');
    expect(screen.queryByRole('listbox')).not.toBeInTheDocument();
    expect(onChange).not.toHaveBeenCalled();
  });
});
