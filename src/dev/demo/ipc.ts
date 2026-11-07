// The IPC stand-in: what `call` gets in a plain browser, in place of the Rust side.
import type { Provider } from '../../shared/types';
import {
  CLI,
  ROOT,
  POWER,
  DEFAULTS,
  CODEX_MODELS,
  CODEX_DEFAULTS,
  CHECKS,
  RECOMMENDED,
  STALE,
  LIBRARY,
  LIBRARY_DIFF,
  CORE,
  BACKGROUND
} from './fixtures';

/** What the stand-in answers with, set when a scene is seeded: until then it
 *  hands every caller its own fallback. */
export const demoState = {
  seeded: false,
  /** Only the scenes about them show any: the README's shots have no such chip. */
  showsChecks: false,
  power: POWER
};

/**
 * Answers the few commands whose emptiness would show. Without this the
 * settings dialog draws no CLI version and no accounts folder in a browser,
 * and a screenshot of it is a picture of blanks. Everything else keeps the
 * caller's own fallback, and outside `?demo` this does nothing at all.
 */
export function answer<T>(cmd: string, args: Record<string, unknown>, fallback: T): T {
  if (!demoState.seeded) return fallback;
  if (cmd === 'core_state') return CORE as unknown as T;
  if (cmd === 'library_overview' || cmd === 'library_sync') return LIBRARY as unknown as T;
  if (cmd === 'library_diff') return LIBRARY_DIFF as unknown as T;
  if (cmd === 'orphan_worktrees') return (STALE[args.folder as string] ?? []).map(w => w.path) as unknown as T;
  if (cmd === 'inspect_worktrees') return (STALE[args.folder as string] ?? []) as unknown as T;
  if (cmd === 'cli_status') return (CLI[args.provider as Provider] ?? CLI.claude) as unknown as T;
  if (cmd === 'get_accounts_root') return ROOT as unknown as T;
  if (cmd === 'claude_defaults') return DEFAULTS as unknown as T;
  if (cmd === 'codex_defaults') return CODEX_DEFAULTS as unknown as T;
  if (cmd === 'codex_models') return CODEX_MODELS as unknown as T;
  if (cmd === 'power_state') return demoState.power as unknown as T;
  if (cmd === 'background_helpers') return BACKGROUND as unknown as T;
  // The Accounts window's own dialogs, so they can be looked at too.
  if (cmd === 'read_account_config') {
    const claudeSide = args.provider === 'claude';
    return {
      path: `${String(args.accountPath)}\\${claudeSide ? 'settings.json' : 'config.toml'}`,
      text: claudeSide
        ? '{\n  "tui": "fullscreen",\n  "spinnerTipsEnabled": false,\n  "env": {\n    "CLAUDE_CODE_DISABLE_FEEDBACK_SURVEY": "1"\n  }\n}\n'
        : '# Luna updates Codex itself\ncheck_for_update_on_startup = false\n\n[tui]\nshow_tooltips = false\n',
      exists: true
    } as unknown as T;
  }
  if (cmd === 'recommended_settings') return RECOMMENDED as unknown as T;
  if (cmd === 'recommendations') return (demoState.showsChecks ? CHECKS : []) as unknown as T;
  if (cmd === 'check_account_config') return null as unknown as T;
  if (cmd === 'settings_sync_state') return { claude: 'personal', codex: null, onStart: true } as unknown as T;
  return fallback;
}
