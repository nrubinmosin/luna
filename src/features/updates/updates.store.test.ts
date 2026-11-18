import { afterAll, afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

/** The release on offer, with a download and an install the test settles. */
const update = vi.hoisted(() => {
  // What has the store take itself to run inside Tauri, as it is read once.
  (window as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ = {};
  return {
    version: '9.9.9',
    body: '',
    rawJson: {},
    download: vi.fn(() => Promise.resolve()),
    install: vi.fn(() => Promise.resolve())
  };
});

vi.mock('@tauri-apps/plugin-updater', () => ({ check: () => Promise.resolve(update) }));
vi.mock('@tauri-apps/api/app', () => ({ getVersion: () => Promise.resolve('1.0.0') }));
vi.mock('../../ipc/coreProcess', () => ({
  noteExit: vi.fn(() => Promise.resolve()),
  stopAllSessions: vi.fn(() => Promise.resolve(0)),
  updateCalledOff: vi.fn(() => Promise.resolve())
}));
vi.mock('../core/core.store', () => ({ useCore: { getState: () => ({ link: { state: 'down' } }) } }));
vi.mock('../../shared/lib/log', () => ({ logInfo() {}, logWarn() {} }));

const { useUpdates } = await import('./updates.store');
const { exiting, throttledStorage } = await import('../../shared/lib/throttledStorage');

const stored = () => (JSON.parse(localStorage.getItem('t') ?? 'null') as { state: { n: number } } | null)?.state.n;
const value = (n: number) => ({ state: { n }, version: 0 });

/** A storage that has just written, so its next change waits for the gap. */
const justWritten = () => {
  const s = throttledStorage<{ n: number }>(JSON.stringify);
  s.setItem('t', value(1));
  return s;
};

beforeEach(async () => {
  vi.stubEnv('VITE_LUNA_FLAVOR', '');
  update.download.mockReset().mockResolvedValue(undefined);
  update.install.mockReset().mockResolvedValue(undefined);
  await useUpdates.getState().check(true);
  expect(useUpdates.getState().phase).toBe('available');
  // The gaps go by the clock, which only the test moves.
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
});

afterAll(() => {
  vi.unstubAllEnvs();
  delete (window as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
});

describe('the updater and the chats storage on the way out', () => {
  it('leaves a tray Restart writing at once when the download fails during its wait', async () => {
    const s = justWritten();
    // The tray's Restart told the page it is going; the download fails
    // within its 300 ms.
    exiting('tray');
    update.download.mockRejectedValue(new Error('offline'));
    await useUpdates.getState().install();
    expect(useUpdates.getState().phase).toBe('error');
    s.setItem('t', value(2));
    expect(stored()).toBe(2);
    exiting('tray', false);
  });

  it('writes at once while its install runs, and with gaps again once it has failed', async () => {
    const s = justWritten();
    let during: number | undefined;
    update.install.mockImplementation(() => {
      s.setItem('t', value(2));
      during = stored();
      return Promise.reject(new Error('no installer'));
    });
    await useUpdates.getState().install();
    expect(during).toBe(2);
    s.setItem('t', value(3));
    expect(stored()).toBe(2);
  });

  it('writes with gaps again once an install returns, where the window stays', async () => {
    const s = justWritten();
    await useUpdates.getState().install();
    s.setItem('t', value(2));
    expect(stored()).toBe(1);
  });
});
