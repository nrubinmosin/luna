import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import * as ipc from '../../ipc/library';
import { AudiencePicker, audienceOf, rolesOf } from './AudiencePicker';

vi.mock('../../ipc/library', async importOriginal => ({
  ...(await importOriginal<typeof import('../../ipc/library')>()),
  librarySetAudience: vi.fn(() => Promise.resolve(null))
}));

describe('AudiencePicker', () => {
  it('shows whose sessions an entry is for and takes a role away', async () => {
    vi.mocked(ipc.librarySetAudience).mockClear();
    const act = vi.fn((p: Promise<unknown>) => p.then(() => true));
    render(<AudiencePicker entry={{ skill: 'council' }} audience="every" act={act} />);
    const boxes = screen.getAllByRole('checkbox');
    expect(boxes.map(r => r.textContent)).toEqual(['Chats', 'Leads', 'Helpers']);
    expect(boxes.every(b => b.getAttribute('aria-checked') === 'true')).toBe(true);
    await userEvent.click(screen.getByRole('checkbox', { name: 'Chats' }));
    expect(ipc.librarySetAudience).toHaveBeenCalledWith({ skill: 'council' }, ['lead', 'helper']);
    expect(act).toHaveBeenCalledTimes(1);
  });

  it('gives a role back, and every one ticked is every session', async () => {
    vi.mocked(ipc.librarySetAudience).mockClear();
    const act = vi.fn((p: Promise<unknown>) => p.then(() => true));
    render(<AudiencePicker entry={{ instructions: 'main' }} audience={['chat', 'helper']} act={act} />);
    expect(screen.getByRole('checkbox', { name: 'Leads' }).getAttribute('aria-checked')).toBe('false');
    await userEvent.click(screen.getByRole('checkbox', { name: 'Leads' }));
    expect(ipc.librarySetAudience).toHaveBeenCalledWith({ instructions: 'main' }, 'every');
  });

  it('never leaves an entry nobody gets', async () => {
    vi.mocked(ipc.librarySetAudience).mockClear();
    const act = vi.fn((p: Promise<unknown>) => p.then(() => true));
    render(<AudiencePicker entry={{ instructions: 'main' }} audience={['chat']} act={act} />);
    const chats = screen.getByRole('checkbox', { name: 'Chats' });
    expect(chats.title).toMatch(/tick another role/);
    await userEvent.click(chats);
    expect(ipc.librarySetAudience).not.toHaveBeenCalled();
    expect(act).not.toHaveBeenCalled();
  });

  it('says on instructions that some roles get them per session, and not on skills', () => {
    const act = vi.fn(() => Promise.resolve(true));
    const { unmount } = render(<AudiencePicker entry={{ instructions: 'main' }} audience="every" act={act} />);
    expect(screen.getByRole('checkbox', { name: 'Chats' }).title).toMatch(/never put into an account/);
    unmount();
    render(<AudiencePicker entry={{ skill: 'council' }} audience="every" act={act} />);
    expect(screen.getByRole('checkbox', { name: 'Chats' }).title).not.toMatch(/account/);
    expect(screen.getByRole('checkbox', { name: 'Chats' }).title).toMatch(/Keep it from your own chats/);
  });

  it('reads and writes an audience as the core does', () => {
    expect(rolesOf('every')).toEqual(['chat', 'lead', 'helper']);
    expect(audienceOf(['helper', 'chat'])).toEqual(['chat', 'helper']);
    expect(audienceOf(['helper', 'lead', 'chat'])).toBe('every');
  });
});
