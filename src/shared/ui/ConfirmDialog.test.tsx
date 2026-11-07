import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { ConfirmDialog } from './ConfirmDialog';

function open(props: Partial<Parameters<typeof ConfirmDialog>[0]> = {}) {
  const onConfirm = vi.fn();
  const onCancel = vi.fn();
  render(
    <ConfirmDialog
      title="Delete chat?"
      body="Its transcript goes too."
      onConfirm={onConfirm}
      onCancel={onCancel}
      {...props}
    />
  );
  return { onConfirm, onCancel, user: userEvent.setup() };
}

describe('ConfirmDialog', () => {
  it('confirms on its confirm button', async () => {
    const { onConfirm, onCancel, user } = open({ confirmLabel: 'Delete for good' });
    await user.click(screen.getByRole('button', { name: 'Delete for good' }));
    expect(onConfirm).toHaveBeenCalledOnce();
    expect(onCancel).not.toHaveBeenCalled();
  });

  it('cancels on Cancel and on the title bar’s close button', async () => {
    const { onConfirm, onCancel, user } = open();
    await user.click(screen.getByRole('button', { name: 'Cancel' }));
    await user.click(screen.getByRole('button', { name: 'Close' }));
    expect(onCancel).toHaveBeenCalledTimes(2);
    expect(onConfirm).not.toHaveBeenCalled();
  });

  it('cancels on Escape and confirms on Enter', async () => {
    const { onConfirm, onCancel, user } = open();
    await user.keyboard('{Escape}');
    expect(onCancel).toHaveBeenCalledOnce();
    await user.keyboard('{Enter}');
    expect(onConfirm).toHaveBeenCalledOnce();
  });

  it('cancels on a click beside the dialog, but not on one inside it', async () => {
    const { onCancel, user } = open();
    await user.click(screen.getByText('Its transcript goes too.'));
    expect(onCancel).not.toHaveBeenCalled();
    await user.click(document.querySelector('.scrim')!);
    expect(onCancel).toHaveBeenCalledOnce();
  });

  it('keeps a dismissing click from reaching the row it was opened from', async () => {
    const onRowClick = vi.fn();
    const onCancel = vi.fn();
    const user = userEvent.setup();
    render(
      <div onClick={onRowClick}>
        <ConfirmDialog title="Delete chat?" body="body" onConfirm={() => {}} onCancel={onCancel} />
      </div>
    );
    await user.click(document.querySelector('.scrim')!);
    expect(onCancel).toHaveBeenCalledOnce();
    expect(onRowClick).not.toHaveBeenCalled();
  });

  it('shows the controls that change what confirming does', () => {
    open({
      extra: (
        <label>
          <input type="checkbox" /> Delete the worktree too
        </label>
      )
    });
    expect(screen.getByRole('checkbox', { name: /delete the worktree too/i })).toBeInTheDocument();
  });

  it('stops listening for keys once closed', async () => {
    const onCancel = vi.fn();
    const user = userEvent.setup();
    const view = render(<ConfirmDialog title="t" body="b" onConfirm={() => {}} onCancel={onCancel} />);
    view.unmount();
    await user.keyboard('{Escape}');
    expect(onCancel).not.toHaveBeenCalled();
  });
});
