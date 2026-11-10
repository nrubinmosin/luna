import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import type { BackgroundHelperDto } from '../../ipc/agents';
import { HelperPanel } from './BackgroundChip';

vi.mock('../../ipc/agents', async importOriginal => ({
  ...(await importOriginal<typeof import('../../ipc/agents')>()),
  backgroundKill: vi.fn(() => Promise.resolve()),
  backgroundDelete: vi.fn(() => Promise.resolve({}))
}));

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

const panel = (helpers: BackgroundHelperDto[]) =>
  render(<HelperPanel helpers={helpers} onWatch={vi.fn()} onOpen={vi.fn()} error={null} setError={vi.fn()} />);

const names = () => [...document.querySelectorAll<HTMLElement>('.helper-row')].map(r => r.dataset.helper);

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
    const review = document.querySelector<HTMLElement>('[data-helper="review"]')!;
    expect(within(review).getByText('Review the release plan')).toHaveAttribute('title', 'Review the release plan');
    expect(within(review).getByText(/active 2m ago/)).toBeTruthy();
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
    const busy = within(document.querySelector<HTMLElement>('[data-helper="busy"]')!);
    expect(busy.getByRole('button', { name: 'Stop' })).toBeEnabled();
    expect(busy.getByRole('button', { name: 'Open as chat' })).toBeDisabled();
    expect(busy.getByRole('button', { name: 'Delete' })).toBeDisabled();
  });
});
