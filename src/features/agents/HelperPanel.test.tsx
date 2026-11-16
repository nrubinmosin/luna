import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { backgroundMove, backgroundMoveTargets, type BackgroundHelperDto } from '../../ipc/agents';
import type { Chat } from '../../shared/types';
import { useChats } from '../chats/chats.store';
import { HelperPanel } from './BackgroundChip';
import { MoveHelperDialog } from './MoveHelperDialog';
import { namer } from './ownership';

// xterm cannot run in jsdom.
vi.mock('../panes/terminals', () => ({
  startSession: vi.fn(),
  focusTerminal: vi.fn(),
  forget: vi.fn(),
  forgetAll: vi.fn()
}));
vi.mock('../../ipc/agents', async importOriginal => ({
  ...(await importOriginal<typeof import('../../ipc/agents')>()),
  backgroundKill: vi.fn(() => Promise.resolve()),
  backgroundDelete: vi.fn(() => Promise.resolve({})),
  backgroundMove: vi.fn(() => Promise.resolve({ id: 'x', from: null, to: 'y', sessions: ['x'] })),
  backgroundMoveTargets: vi.fn(() => Promise.resolve({ targets: [] }))
}));

const chat = (id: string, name: string) => ({ id, name, provider: 'claude', group: 0 }) as Chat;
beforeEach(() => {
  vi.mocked(backgroundMove).mockClear();
  useChats.setState({
    folders: [{ id: 'f', path: '/repo', open: true, chats: [chat('chat', 'Release'), chat('docs', 'Docs')] }]
  });
});

const row = (id: string, state: string, over: Partial<BackgroundHelperDto> = {}): BackgroundHelperDto => ({
  id,
  parent: 'chat',
  name: id,
  provider: 'claude',
  state,
  account: 'work',
  model: 'haiku',
  effort: 'low',
  worktree: false,
  startedMs: Date.now() - 600_000,
  turnStartedMs: null,
  lastActivityMs: Date.now() - 120_000,
  task: null,
  ...over
});

const panel = (helpers: BackgroundHelperDto[], onMove = vi.fn()) =>
  render(
    <HelperPanel helpers={helpers} onWatch={vi.fn()} onOpen={vi.fn()} onMove={onMove} error={null} setError={vi.fn()} />
  );

/** The panel with its error kept, as the list's owner keeps it. */
function Kept({ helpers }: { helpers: BackgroundHelperDto[] }) {
  const [error, setError] = useState<string | null>(null);
  return (
    <HelperPanel
      helpers={helpers}
      onWatch={vi.fn()}
      onOpen={vi.fn()}
      onMove={vi.fn()}
      error={error}
      setError={setError}
    />
  );
}

const names = () => [...document.querySelectorAll<HTMLElement>('.helper-row')].map(r => r.dataset.helper);
const rowOf = (id: string) => within(document.querySelector<HTMLElement>(`[data-helper="${id}"]`)!);

describe('HelperPanel', () => {
  it('opens on Working, the asking first and marked, with each row’s task and last activity', () => {
    panel([
      row('review', 'working', { task: 'Review the release plan' }),
      row('done', 'idle'),
      row('asks', 'question', { ask: { kind: 'question', blocking: true, questions: [{ text: 'Ship tonight?' }] } })
    ]);
    expect(screen.getByRole('tab', { name: 'Working 2' })).toHaveAttribute('aria-selected', 'true');
    expect(screen.getByRole('tab', { name: 'Idle 1' })).toHaveAttribute('aria-selected', 'false');
    expect(names()).toEqual(['asks', 'review']);
    expect(document.querySelector('[data-helper="asks"]')).toHaveAttribute('data-asking', 'yes');
    const review = rowOf('review');
    expect(review.getByText('Review the release plan')).toHaveAttribute('title', 'Review the release plan');
    expect(review.getByText(/active 2m ago/)).toBeTruthy();
    expect(screen.getByText('Ship tonight?')).toBeTruthy();
  });

  it('opens on Idle when nothing works, and switches on a tab', async () => {
    const user = userEvent.setup();
    panel([row('done', 'idle'), row('gone', 'exited')]);
    expect(screen.getByRole('tab', { name: 'Idle 2' })).toHaveAttribute('aria-selected', 'true');
    expect(names()).toEqual(['done', 'gone']);
    await user.click(screen.getByRole('tab', { name: 'Working 0' }));
    expect(names()).toEqual([]);
    expect(screen.getByText('None at work.')).toBeTruthy();
  });

  it('keeps each helper’s buttons: Open as chat and Delete only between turns, Stop only in one', () => {
    panel([row('busy', 'working'), row('done', 'idle')]);
    const busy = rowOf('busy');
    expect(busy.getByRole('button', { name: 'Stop' })).toBeEnabled();
    expect(busy.getByRole('button', { name: 'Open as chat' })).toBeDisabled();
    expect(busy.getByRole('button', { name: 'Delete' })).toBeDisabled();
    // A move does not wait for its turn to end.
    expect(busy.getByRole('button', { name: 'Move to chat…' })).toBeEnabled();
  });

  it('marks whose each is, from its row: who manages it, who watches it, where it was moved from', () => {
    panel([
      row('lead', 'working', {
        top: 'chat',
        watchedBy: ['docs'],
        moved: { from: 'docs', to: 'chat', atMs: Date.now() - 5 * 60_000, told: true }
      }),
      row('kid', 'working', { parent: 'lead', top: 'chat' })
    ]);
    expect(rowOf('lead').getByTestId('helper-marks')).toHaveTextContent(
      'watched by Docs · moved from Docs, 5m ago · managed by chat Release'
    );
    expect(rowOf('kid').getByTestId('helper-marks')).toHaveTextContent('managed by lead · chat Release');
    expect(screen.queryByRole('button', { name: /^Move to .*\?$/ })).toBeNull();
  });

  it('moves a helper to a chat that asked only on a click, and says why when the core refuses', async () => {
    const user = userEvent.setup();
    render(<Kept helpers={[row('lead', 'idle', { top: 'chat', suggested: [{ chat: 'docs', atMs: 1 }] })]} />);
    expect(backgroundMove).not.toHaveBeenCalled();
    vi.mocked(backgroundMove).mockRejectedValueOnce(`chat "Docs" (docs) runs without Luna's tools`);
    await user.click(screen.getByRole('button', { name: 'Move to Docs?' }));
    expect(backgroundMove).toHaveBeenCalledWith('lead', 'docs');
    expect(await screen.findByText(/runs without Luna's tools/)).toBeTruthy();
  });

  it('offers Move to chat… for a chat’s own helper and not for a helper’s helper, which goes with the one above', async () => {
    const user = userEvent.setup();
    const onMove = vi.fn();
    panel([row('lead', 'idle'), row('kid', 'idle', { parent: 'lead' })], onMove);
    const kid = rowOf('kid').getByRole('button', { name: 'Move to chat…' });
    expect(kid).toBeDisabled();
    expect(kid).toHaveAttribute('title', 'It moves with "lead", the helper above it');
    await user.click(rowOf('lead').getByRole('button', { name: 'Move to chat…' }));
    expect(onMove).toHaveBeenCalledWith(expect.objectContaining({ id: 'lead' }));
  });
});

describe('MoveHelperDialog', () => {
  const nameOf = namer([chat('chat', 'Release'), chat('docs', 'Docs'), chat('bare', 'Bare')], []);

  it('lists the chats, the ones it may not go to disabled with why, and moves only on Move', async () => {
    const user = userEvent.setup();
    vi.mocked(backgroundMoveTargets).mockResolvedValueOnce({
      targets: [
        { id: 'chat', ok: false, why: 'lead is a helper of chat "Release" already' },
        { id: 'docs', ok: true },
        { id: 'bare', ok: false, why: `chat "Bare" (bare) runs without Luna's tools` }
      ]
    });
    const onClose = vi.fn();
    render(<MoveHelperDialog helper={row('lead', 'idle')} nameOf={nameOf} onClose={onClose} />);
    expect(await screen.findByLabelText('Docs')).toBeEnabled();
    expect(screen.getByLabelText('Bare')).toBeDisabled();
    expect(screen.getByText(/runs without Luna's tools/)).toBeTruthy();
    // Its own chat is not offered.
    expect(screen.queryByLabelText('Release')).toBeNull();
    expect(screen.getByRole('button', { name: 'Move' })).toBeDisabled();
    await user.click(screen.getByLabelText('Docs'));
    expect(backgroundMove).not.toHaveBeenCalled();
    await user.click(screen.getByRole('button', { name: 'Move' }));
    expect(backgroundMove).toHaveBeenCalledWith('lead', 'docs');
    await waitFor(() => expect(onClose).toHaveBeenCalled());
  });

  it('starts on a chat that asked for it, and keeps a refusal in the dialog', async () => {
    const user = userEvent.setup();
    vi.mocked(backgroundMoveTargets).mockResolvedValueOnce({ targets: [{ id: 'docs', ok: true }] });
    vi.mocked(backgroundMove).mockRejectedValueOnce('lead is held back (opening as a chat); not moved');
    const onClose = vi.fn();
    const asked = row('lead', 'idle', { suggested: [{ chat: 'docs', atMs: 1 }] });
    render(<MoveHelperDialog helper={asked} nameOf={nameOf} onClose={onClose} />);
    expect(await screen.findByLabelText('Docs')).toBeChecked();
    expect(backgroundMove).not.toHaveBeenCalled();
    await user.click(screen.getByRole('button', { name: 'Move' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('Not moved: lead is held back');
    expect(onClose).not.toHaveBeenCalled();
  });

  it('says why when it cannot move at all', async () => {
    vi.mocked(backgroundMoveTargets).mockResolvedValueOnce({
      blocked: 'lead is held back (being deleted); not moved',
      targets: []
    });
    render(<MoveHelperDialog helper={row('lead', 'idle')} nameOf={nameOf} onClose={vi.fn()} />);
    expect(await screen.findByTestId('move-blocked')).toHaveTextContent('being deleted');
    expect(screen.getByRole('button', { name: 'Move' })).toBeDisabled();
  });
});
