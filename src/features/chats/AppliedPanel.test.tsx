import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import * as ipc from '../../ipc/applied';
import type { Applied } from '../../ipc/applied';
import * as handoffIpc from '../../ipc/handoff';
import type { Chat } from '../../shared/types';
import { AppliedPanel, byClass } from './AppliedPanel';
import { useChats } from './chats.store';

vi.mock('../../ipc/applied', async importOriginal => ({
  ...(await importOriginal<typeof import('../../ipc/applied')>()),
  appliedByLuna: vi.fn()
}));
vi.mock('../../ipc/handoff', () => ({
  setAutoHandoff: vi.fn(() => Promise.resolve({ resume: false })),
  handoffFile: vi.fn(),
  handoffTranscript: vi.fn()
}));
// xterm cannot run in jsdom.
vi.mock('../panes/terminals', () => ({
  startSession: vi.fn(),
  focusTerminal: vi.fn(),
  forget: vi.fn(),
  forgetAll: vi.fn()
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
      account: [item('trust', 'userAction', 'E:\\src\\app')],
      incomplete: null,
      handoff: null
    });
    render(<AppliedPanel chat={chat} onClose={() => {}} />);
    await waitFor(() => expect(screen.getByText('permissions.deny Read(//e/Luna/core/**)')).toBeInTheDocument());
    expect(ipc.appliedByLuna).toHaveBeenCalledWith('c1');
    expect(screen.getByText('Protects Luna’s own boundary')).toBeInTheDocument();
    expect(screen.getByText('Why core-read-deny.')).toBeInTheDocument();
    expect(screen.getByText('Written into the account “work”')).toBeInTheDocument();
    expect(screen.getByText('E:\\src\\app')).toBeInTheDocument();
  });

  it('says the account part may be short when the ledger is', async () => {
    vi.mocked(ipc.appliedByLuna).mockResolvedValue({
      launch: [item('model', 'userAction', '--model opus')],
      account: [],
      incomplete: 'trust /p in /acc was not noted: disk full',
      handoff: null
    });
    render(<AppliedPanel chat={chat} onClose={() => {}} />);
    await waitFor(() => expect(screen.getByRole('alert')).toHaveTextContent('Accounting incomplete.'));
    expect(screen.getByRole('alert')).toHaveTextContent('was not noted: disk full');
  });

  it('shows the handoff instructions once auto-handoff is ticked, without being opened again', async () => {
    const tooled = { ...chat, tools: true, status: 'resting', group: 0 } as Chat;
    useChats.setState({ folders: [{ id: 'f', path: '/repo', open: true, chats: [tooled] }] });
    const none = { launch: [], account: [], incomplete: null, handoff: null };
    vi.mocked(ipc.appliedByLuna)
      .mockReset()
      .mockResolvedValueOnce(none)
      .mockResolvedValue({
        ...none,
        handoff: {
          instructions: {
            name: 'session-handoff',
            state: 'luna',
            version: '0.12.0',
            sha256: '3054a9e14c774f592c9c2fed08da4ff7eb38befdb00321184f3d1b324c8aedd9',
            path: '/lib/system/session-handoff/SKILL.md',
            problem: null
          },
          sameNamed: []
        }
      });
    render(<AppliedPanel chat={tooled} onClose={() => {}} />);
    await waitFor(() => expect(screen.getByText(/has not started under this Luna/)).toBeInTheDocument());
    expect(screen.queryByText(/Handoff instructions/)).toBeNull();
    await userEvent.click(screen.getByLabelText('Auto-handoff at'));
    expect(await screen.findByText('Handoff instructions: Luna’s (0.12.0, sha 3054a9e14c77)')).toBeInTheDocument();
    // Asked again only once the core had taken the change.
    const [setAt] = vi.mocked(handoffIpc.setAutoHandoff).mock.invocationCallOrder;
    const [, askedAgain] = vi.mocked(ipc.appliedByLuna).mock.invocationCallOrder;
    expect(setAt).toBeLessThan(askedAgain!);
  });

  it('says so when the chat has not started, and closes', async () => {
    vi.mocked(ipc.appliedByLuna).mockResolvedValue({ launch: [], account: [], incomplete: null, handoff: null });
    const onClose = vi.fn();
    const user = userEvent.setup();
    render(<AppliedPanel chat={chat} onClose={onClose} />);
    await waitFor(() => expect(screen.getByText(/has not started under this Luna/)).toBeInTheDocument());
    await user.click(screen.getByText('Close', { selector: 'button' }));
    expect(onClose).toHaveBeenCalled();
  });
});
