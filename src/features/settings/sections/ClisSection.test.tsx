import { act, render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { CliStatusDto, CliVersionsDto } from '../../../ipc/cli';
import { useCli } from '../../updates/cli.store';
import { ClisSection } from './ClisSection';

const calls: unknown[][] = [];
vi.mock('../../../ipc/cli', async importOriginal => ({
  ...(await importOriginal<typeof import('../../../ipc/cli')>()),
  cliVersions: () => Promise.resolve(null),
  cliUseVersion: (...a: unknown[]) => (calls.push(['use', ...a]), Promise.resolve()),
  cliMarkBad: (...a: unknown[]) => (calls.push(['bad', ...a]), Promise.resolve()),
  cliMarkGood: (...a: unknown[]) => (calls.push(['good', ...a]), Promise.resolve()),
  cliClearMark: (...a: unknown[]) => (calls.push(['clear', ...a]), Promise.resolve()),
  cliSetChannel: (...a: unknown[]) => (calls.push(['channel', ...a]), Promise.resolve()),
  cliCanaryDismiss: (...a: unknown[]) => (calls.push(['dismiss', ...a]), Promise.resolve()),
  cliUpdateNow: (...a: unknown[]) => (calls.push(['check', ...a]), Promise.resolve())
}));

const status = (provider: 'claude' | 'codex', version: string): CliStatusDto => ({
  provider,
  phase: 'idle',
  version,
  path: '',
  latest: version,
  channel: 'latest',
  got: 0,
  total: null,
  error: null,
  note: null,
  checkedAtMs: Date.now()
});

const CLAUDE: CliVersionsDto = {
  provider: 'claude',
  channel: 'latest',
  versions: [
    {
      dir: '2.1.340',
      version: '2.1.340',
      current: true,
      previous: false,
      inUse: true,
      knownGood: false,
      bad: null,
      good: null
    },
    {
      dir: '2.1.319',
      version: '2.1.319',
      current: false,
      previous: true,
      inUse: true,
      knownGood: true,
      bad: null,
      good: null
    },
    {
      dir: '2.1.300',
      version: '2.1.300',
      current: false,
      previous: false,
      inUse: false,
      knownGood: false,
      bad: 'hangs',
      good: null
    }
  ],
  knownGood: ['2.1.319'],
  minSafe: '2.1.319',
  floor: '2.1.340',
  pinned: null,
  bad: { '2.1.300': 'hangs', '2.1.200': 'failed the offline probe: x' },
  good: {},
  canary: { version: '2.1.340', watched: 3, settled: 1, failures: ['a', 'b'], warned: true, dismissed: false }
};

const CODEX: CliVersionsDto = {
  ...CLAUDE,
  provider: 'codex',
  channel: 'knownGood',
  versions: [
    {
      dir: '0.167.0',
      version: '0.167.0',
      current: true,
      previous: false,
      inUse: false,
      knownGood: true,
      bad: null,
      good: null
    }
  ],
  pinned: '0.167.0',
  bad: {},
  canary: null
};

beforeEach(() => {
  calls.length = 0;
  act(() =>
    useCli.setState({
      status: { claude: status('claude', '2.1.340'), codex: { ...status('codex', '0.167.0'), channel: 'knownGood' } },
      versions: { claude: CLAUDE, codex: CODEX }
    })
  );
});

describe('ClisSection', () => {
  it('offers the rollback a canary warning calls for, and keeping the version', async () => {
    render(<ClisSection />);
    const user = userEvent.setup();
    expect(screen.getByText('Claude Code 2.1.340 may be broken')).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Roll back to 2.1.319' }));
    await user.click(screen.getByRole('button', { name: 'Keep it' }));
    expect(calls).toEqual([
      ['use', 'claude', '2.1.319'],
      ['dismiss', 'claude']
    ]);
  });

  it('lists each version with what it is, and uses or marks one', async () => {
    render(<ClisSection />);
    const user = userEvent.setup();
    const list = within(screen.getByRole('list', { name: 'Claude Code versions' }));
    const items = list.getAllByRole('listitem');
    expect(items.map(i => i.querySelector('.set-item-name')?.textContent)).toEqual([
      '2.1.340current',
      '2.1.319previousknown-goodin use',
      '2.1.300bad'
    ]);
    // The current one has no Use; a bad one is cleared, not marked; a
    // listed one is good already.
    expect(within(items[0]!).getByRole('button', { name: 'Use' })).toBeDisabled();
    expect(within(items[2]!).queryByRole('button', { name: 'Mark bad' })).toBeNull();
    expect(within(items[1]!).queryByRole('button', { name: 'Mark good' })).toBeNull();
    await user.click(within(items[2]!).getByRole('button', { name: 'Use' }));
    await user.click(within(items[1]!).getByRole('button', { name: 'Mark bad' }));
    await user.click(within(items[0]!).getByRole('button', { name: 'Mark good' }));
    await user.click(within(items[2]!).getByRole('button', { name: 'Clear bad' }));
    expect(calls).toEqual([
      ['use', 'claude', '2.1.300'],
      ['bad', 'claude', '2.1.319'],
      ['good', 'claude', '2.1.340'],
      ['clear', 'claude', '2.1.300']
    ]);
  });

  it('keeps a mark on a version no longer installed within reach', async () => {
    render(<ClisSection />);
    const user = userEvent.setup();
    const list = within(screen.getByRole('list', { name: 'Claude Code marks on versions not installed' }));
    expect(list.getAllByRole('listitem').map(i => i.querySelector('.set-item-name')?.textContent)).toEqual([
      '2.1.200bad'
    ]);
    await user.click(list.getByRole('button', { name: 'Clear bad' }));
    expect(calls).toEqual([['clear', 'claude', '2.1.200']]);
  });

  it('says a version got through its first sessions only once they all settled with none failed', () => {
    const through = { ...CLAUDE, canary: { ...CLAUDE.canary!, settled: 3, failures: [], warned: false } };
    act(() => useCli.setState({ versions: { claude: through, codex: CODEX } }));
    const { unmount } = render(<ClisSection />);
    const first = within(screen.getByRole('list', { name: 'Claude Code versions' })).getAllByRole('listitem')[0]!;
    expect(first.querySelector('.set-item-name')?.textContent).toBe('2.1.340currentfirst 3 sessions ok');
    unmount();
    const started = { ...through, canary: { ...through.canary, settled: 2 } };
    act(() => useCli.setState({ versions: { claude: started, codex: CODEX } }));
    render(<ClisSection />);
    const again = within(screen.getByRole('list', { name: 'Claude Code versions' })).getAllByRole('listitem')[0]!;
    expect(again.querySelector('.set-item-name')?.textContent).toBe('2.1.340current');
  });

  it('switches the channel, and lets go of a version held by hand', async () => {
    render(<ClisSection />);
    const user = userEvent.setup();
    await user.selectOptions(screen.getByRole('combobox', { name: 'Claude Code channel' }), 'knownGood');
    expect(screen.getByText(/Held at 0\.167\.0 by hand/)).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Follow channel' }));
    expect(calls).toEqual([
      ['channel', 'claude', 'knownGood'],
      ['channel', 'codex', 'knownGood']
    ]);
  });
});
