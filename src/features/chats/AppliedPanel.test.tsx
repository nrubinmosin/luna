import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import * as ipc from '../../ipc/applied';
import type { Applied } from '../../ipc/applied';
import type { Chat } from '../../shared/types';
import { AppliedPanel, byClass } from './AppliedPanel';

vi.mock('../../ipc/applied', async importOriginal => ({
  ...(await importOriginal<typeof import('../../ipc/applied')>()),
  appliedByLuna: vi.fn()
}));

const item = (key: string, cls: Applied['class'], value = key): Applied => ({
  key,
  class: cls,
  value,
  why: `Why ${key}.`
});

const chat = { id: 'c1', name: 'fix the build', account: 'work', provider: 'claude' } as Chat;

describe('AppliedPanel', () => {
  it('groups what was applied by why it is there, the person’s choices first', () => {
    const groups = byClass([
      item('activity-hooks', 'runtime'),
      item('codex.sandbox-path', 'workaround'),
      item('model', 'userAction'),
      item('core-read-deny', 'boundary'),
      item('session-id', 'runtime')
    ]);
    expect(groups.map(g => [g.cls, g.items.map(i => i.key)])).toEqual([
      ['userAction', ['model']],
      ['runtime', ['activity-hooks', 'session-id']],
      ['boundary', ['core-read-deny']],
      ['workaround', ['codex.sandbox-path']]
    ]);
    expect(byClass([item('model', 'userAction')]).map(g => g.cls)).toEqual(['userAction']);
  });

  it('shows each item with its value and why, and the account’s writes apart', async () => {
    vi.mocked(ipc.appliedByLuna).mockResolvedValue({
      launch: [item('core-read-deny', 'boundary', 'permissions.deny Read(//e/Luna/core/**)')],
      account: [item('trust', 'userAction', 'E:\\src\\app')]
    });
    render(<AppliedPanel chat={chat} onClose={() => {}} />);
    await waitFor(() => expect(screen.getByText('permissions.deny Read(//e/Luna/core/**)')).toBeInTheDocument());
    expect(ipc.appliedByLuna).toHaveBeenCalledWith('c1');
    expect(screen.getByText('Protects Luna’s own boundary')).toBeInTheDocument();
    expect(screen.getByText('Why core-read-deny.')).toBeInTheDocument();
    expect(screen.getByText('Written into the account “work”')).toBeInTheDocument();
    expect(screen.getByText('E:\\src\\app')).toBeInTheDocument();
  });

  it('says so when the chat has not started, and closes', async () => {
    vi.mocked(ipc.appliedByLuna).mockResolvedValue({ launch: [], account: [] });
    const onClose = vi.fn();
    const user = userEvent.setup();
    render(<AppliedPanel chat={chat} onClose={onClose} />);
    await waitFor(() => expect(screen.getByText(/has not started under this Luna/)).toBeInTheDocument());
    await user.click(screen.getByText('Close', { selector: 'button' }));
    expect(onClose).toHaveBeenCalled();
  });
});
