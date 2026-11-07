import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

type Handler = (cmd: string, args: Record<string, unknown>) => unknown;

/** What the window asked of Rust, in order. */
let asked: Array<{ cmd: string; args: Record<string, unknown> }> = [];

/**
 * The commands module as it loads inside Tauri. It decides once, as it loads,
 * whether it runs there (and falls back to the demo when not), so the mock
 * goes in first and the module is loaded fresh after it.
 */
async function inTauri(answer: Handler) {
  asked = [];
  mockIPC((cmd, args) => {
    const a = (args ?? {}) as Record<string, unknown>;
    asked.push({ cmd, args: a });
    // The log line the module writes for a failure goes to Rust too.
    if (cmd === 'append_log') return undefined;
    return answer(cmd, a);
  });
  vi.resetModules();
  return import('./commands');
}

/** A command that fails the way Tauri's do: with the Rust error as a plain string. */
const failing = (error: string) => () => {
  // eslint-disable-next-line @typescript-eslint/only-throw-error -- what Rust's Err(String) arrives as
  throw error;
};

/** The lines written to Luna's log through Rust. */
const logged = () => asked.filter(a => a.cmd === 'append_log').map(a => String(a.args.message));

beforeEach(() => {
  vi.spyOn(console, 'warn').mockImplementation(() => {});
});

afterEach(() => {
  clearMocks();
  // clearMocks empties the internals but leaves them there, and their being
  // there is what makes the module think it runs inside Tauri.
  delete (window as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
  vi.restoreAllMocks();
});

describe('ipc commands inside Tauri', () => {
  it('returns what the command answers', async () => {
    const account = { provider: 'claude', name: 'work', path: '/acc/work' };
    const ipc = await inTauri(cmd => (cmd === 'list_accounts' ? [account] : null));
    await expect(ipc.listAccounts()).resolves.toEqual([account]);
    expect(asked[0]).toEqual({ cmd: 'list_accounts', args: {} });
  });

  it('passes a session command through the window to the core', async () => {
    const ipc = await inTauri(() => undefined);
    await ipc.killSession('c1');
    expect(asked[0]).toEqual({ cmd: 'core_call', args: { cmd: 'kill_session', args: { id: 'c1' } } });
  });

  it('sends keystrokes to the core without waiting on an answer of their own', async () => {
    const ipc = await inTauri(() => undefined);
    await ipc.writeSession('c1', 'ls\r');
    expect(asked[0]).toEqual({ cmd: 'core_send', args: { cmd: 'write_session', args: { id: 'c1', data: 'ls\r' } } });
  });

  it('puts a Claude session’s model and permission mode in the CLI’s own words', async () => {
    const ipc = await inTauri(() => 'session');
    await ipc.ensureClaudeSession({
      chatId: 'c1',
      folder: '/repo',
      accountPath: '/acc/work',
      model: 'Sonnet',
      effort: 'high',
      perm: 'Plan only',
      worktree: false
    });
    const inner = asked[0]!.args as { cmd: string; args: Record<string, unknown> };
    expect(inner.cmd).toBe('ensure_claude_session');
    expect(inner.args).toMatchObject({ id: 'c1', model: 'sonnet', permissionMode: 'plan', resume: null, tools: false });
  });

  it('rejects with the error the command failed with, and logs it', async () => {
    const ipc = await inTauri(failing('account exists'));
    await expect(ipc.createAccount('claude', 'work')).rejects.toBe('account exists');
    expect(logged()).toEqual(['create_account failed: account exists']);
  });

  it('does not log every call while the core is not running: the banner says so', async () => {
    const ipc = await inTauri(failing('the core is not running'));
    await expect(ipc.killSession('c1')).rejects.toBe('the core is not running');
    expect(logged()).toEqual([]);
  });

  it('leaves logging a failed usage request to Rust, which says more', async () => {
    const ipc = await inTauri(failing('HTTP 500'));
    await expect(ipc.accountLimits('/acc/work')).rejects.toBe('HTTP 500');
    expect(logged()).toEqual([]);
  });
});

describe('ipc commands outside Tauri', () => {
  it('hands back each command’s empty answer, so the UI still works in a plain browser', async () => {
    vi.resetModules();
    const ipc = await import('./commands');
    await expect(ipc.listAccounts()).resolves.toEqual([]);
    await expect(ipc.accountLimits('/acc/work')).resolves.toBeNull();
    await expect(ipc.claudeFolderTrusted('/acc/work', '/repo')).resolves.toBe(true);
  });
});
