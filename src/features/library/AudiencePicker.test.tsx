import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import * as ipc from '../../ipc/library';
import { AudiencePicker } from './AudiencePicker';

vi.mock('../../ipc/library', async importOriginal => ({
  ...(await importOriginal<typeof import('../../ipc/library')>()),
  librarySetAudience: vi.fn(() => Promise.resolve(null))
}));

describe('AudiencePicker', () => {
  it('shows whose sessions an entry is for and sets another', async () => {
    const act = vi.fn((p: Promise<unknown>) => p.then(() => true));
    render(<AudiencePicker entry={{ skill: 'council' }} audience="both" act={act} />);
    const radios = screen.getAllByRole('radio');
    expect(radios.map(r => r.textContent)).toEqual(['Both', 'Chats', 'Helpers']);
    expect(screen.getByRole('radio', { checked: true }).textContent).toBe('Both');
    await userEvent.click(screen.getByRole('radio', { name: 'Chats' }));
    expect(ipc.librarySetAudience).toHaveBeenCalledWith({ skill: 'council' }, 'chats');
    expect(act).toHaveBeenCalledTimes(1);
  });

  it('sends nothing for the audience it already has', async () => {
    vi.mocked(ipc.librarySetAudience).mockClear();
    const act = vi.fn((p: Promise<unknown>) => p.then(() => true));
    render(<AudiencePicker entry={{ instructions: 'main' }} audience="chats" act={act} />);
    await userEvent.click(screen.getByRole('radio', { name: 'Chats' }));
    expect(ipc.librarySetAudience).not.toHaveBeenCalled();
    expect(act).not.toHaveBeenCalled();
  });

  it('says on instructions that Codex copies say whose they are, and not on skills', () => {
    const act = vi.fn(() => Promise.resolve(true));
    const { unmount } = render(<AudiencePicker entry={{ instructions: 'main' }} audience="both" act={act} />);
    expect(screen.getByRole('radio', { name: 'Chats' }).title).toMatch(/AGENTS\.md in every session/);
    unmount();
    render(<AudiencePicker entry={{ skill: 'council' }} audience="both" act={act} />);
    expect(screen.getByRole('radio', { name: 'Chats' }).title).not.toMatch(/AGENTS\.md/);
    expect(screen.getByRole('radio', { name: 'Chats' }).title).toMatch(/keeps it from helpers/);
  });
});
