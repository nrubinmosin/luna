import { invoke } from '@tauri-apps/api/core';
import { logWarn, SLOW_MS } from '../shared/lib/log';

export const tauriAvailable = '__TAURI_INTERNALS__' in window;

// Commands that do their real work off the main thread. A slow one means a slow
// network or a big transcript — not a frozen window — so they get a budget of
// their own rather than tripping the stall warning thousands of times a day.
const SLOW_BUDGET_MS: Record<string, number> = {
  // A usage request may wait out a 10s read timeout, and a token renewal before it.
  account_limits: 15_000,
  codex_limits: 15_000,
  session_meta: 2000,
  saved_title: 5000,
  ensure_claude_session: 30_000,
  ensure_codex_session: 30_000,
  // Hashes every file of the version again: Codex's are half a gigabyte.
  cli_use_version: 20_000,
  // Reads every account's files, and looks twice at a blank one.
  recommendations: 5000,
  apply_recommendation: 5000,
  // Both give the CLI a moment to quit on its own before killing it; deleting
  // then waits for the worktree to come free.
  kill_session: 5000,
  delete_session: 20_000,
  orphan_worktrees: 10_000,
  remove_orphan_worktrees: 30_000,
  // Reads a transcript's ends and asks git, each bounded; 30 s in all.
  move_prepare: 35_000
};

// What the core answers (corecmd.rs): everything that touches the running
// sessions, which live in the core process rather than the window's. The
// window passes them on; keystrokes and resizes go without waiting for an
// answer, in the order they are made.
const CORE_CALL: ReadonlySet<string> = new Set([
  'ensure_claude_session',
  'ensure_codex_session',
  'kill_session',
  'delete_session',
  'stop_all_sessions',
  'orphan_sessions',
  'session_meta',
  'session_modes',
  'dismiss_waiting',
  'chat_names',
  'account_usage',
  'power_state',
  'background_helpers',
  'background_promote',
  'background_promoted',
  'background_kill',
  'background_delete',
  'background_at_stake',
  'background_transcript',
  'set_keep_awake',
  'arm_power_off',
  'disarm_power_off',
  'rename_account',
  'orphan_worktrees',
  'remove_orphan_worktrees',
  'sync_settings_now',
  'recommendations',
  'apply_recommendation',
  'dismiss_recommendation',
  'apply_recommended',
  'codex_folder_trusted',
  'codex_trust_folder',
  'claude_trust_folder',
  'applied_by_luna',
  'set_auto_handoff',
  'handoff_now',
  'handoff_file',
  'handoff_transcript',
  'move_prepare',
  'move_confirm',
  'move_step',
  'move_cancel',
  'move_list',
  'move_ask',
  'move_preview',
  'move_build_from_records',
  'move_facts'
]);
const CORE_SEND: ReadonlySet<string> = new Set(['write_session', 'resize_session']);

const invokeAnywhere = <T>(cmd: string, args?: Record<string, unknown>): Promise<T> =>
  CORE_SEND.has(cmd)
    ? invoke<T>('core_send', { cmd, args: args ?? {} })
    : CORE_CALL.has(cmd)
      ? invoke<T>('core_call', { cmd, args: args ?? {} })
      : invoke<T>(cmd, args);

// The Rust side already logs why these failed, with more to say than this
// layer has; logging them here as well wrote every failure twice.
const LOGGED_BEHIND: ReadonlySet<string> = new Set(['account_limits', 'codex_limits']);

// In plain-browser dev (vite without tauri) all commands become no-ops so the
// UI stays workable.
export async function call<T>(cmd: string, args?: Record<string, unknown>, fallback?: T): Promise<T> {
  if (!tauriAvailable) {
    // The README fixture answers the handful of commands whose emptiness would
    // show in a screenshot; without `?demo` it hands back the same fallback as
    // ever. Compiled out of a release build along with the module.
    if (import.meta.env.DEV) return (await import('../dev/demo/ipc')).answer(cmd, args ?? {}, fallback as T);
    return fallback as T;
  }
  // Timed because a sync Tauri command runs on the main thread: a slow one is
  // indistinguishable from the app hanging, and this names which one it was.
  const started = performance.now();
  try {
    return await invokeAnywhere<T>(cmd, args);
  } catch (e) {
    // A core that is not there fails every call until it is back, and the
    // banner already says so; one line per poll would bury the log.
    const noCore = String(e).includes('the core is not running');
    if (!LOGGED_BEHIND.has(cmd) && !noCore) logWarn('ipc', `${cmd} failed: ${String(e)}`);
    throw e;
  } finally {
    const ms = performance.now() - started;
    if (ms > (SLOW_BUDGET_MS[cmd] ?? SLOW_MS)) logWarn('ipc', `${cmd} took ${Math.round(ms)}ms`);
  }
}
