// The IPC stand-in: what `call` gets in a plain browser, in place of the Rust side.
import type { Provider } from '../../shared/types';
import {
  CLI,
  CLI_VERSIONS,
  ROOT,
  POWER,
  DEFAULTS,
  CLAUDE_MODELS,
  CODEX_MODELS,
  CODEX_DEFAULTS,
  CHECKS,
  RECOMMENDED,
  STALE,
  LIBRARY,
  LIBRARY_DIFF,
  SYSTEM_COMPARE,
  CORE,
  BACKGROUND,
  HELPER_TRANSCRIPT,
  HANDOFF_FILE,
  AT_STAKE,
  APPLIED,
  BROWSERS,
  MOVE_PREVIEW,
  ORPHANS
} from './fixtures';

/** What the stand-in answers with, set when a scene is seeded: until then it
 *  hands every caller its own fallback. */
export const demoState = {
  seeded: false,
  /** Only the scenes about them show any: the README's shots have no such chip. */
  showsChecks: false,
  /** Only Settings → CLIs shows a canary warning, and its chip with it. */
  showsCanary: false,
  /** Only the scene about them has sessions with no chat. */
  showsOrphans: false,
  /** The library's scene with an override of a system skill shows that library. */
  library: LIBRARY,
  /** What the Claude settings files say: `newchat-more` names an older model. */
  claudeDefaults: DEFAULTS,
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
  if (cmd === 'library_overview' || cmd === 'library_sync') return demoState.library as unknown as T;
  if (cmd === 'library_diff') return LIBRARY_DIFF as unknown as T;
  if (cmd === 'library_system_compare') return SYSTEM_COMPARE as unknown as T;
  if (cmd === 'orphan_worktrees') return (STALE[args.folder as string] ?? []).map(w => w.path) as unknown as T;
  if (cmd === 'inspect_worktrees') return (STALE[args.folder as string] ?? []) as unknown as T;
  if (cmd === 'cli_status') return (CLI[args.provider as Provider] ?? CLI.claude) as unknown as T;
  if (cmd === 'cli_versions') {
    const v = CLI_VERSIONS[args.provider as Provider] ?? CLI_VERSIONS.claude;
    return (demoState.showsCanary ? v : { ...v, canary: null }) as unknown as T;
  }
  if (cmd === 'get_accounts_root') return ROOT as unknown as T;
  if (cmd === 'claude_defaults') return demoState.claudeDefaults as unknown as T;
  if (cmd === 'claude_models') return CLAUDE_MODELS as unknown as T;
  if (cmd === 'codex_defaults') return CODEX_DEFAULTS as unknown as T;
  if (cmd === 'codex_models') return CODEX_MODELS as unknown as T;
  if (cmd === 'power_state') return demoState.power as unknown as T;
  if (cmd === 'orphan_sessions') return (demoState.showsOrphans ? ORPHANS : []) as unknown as T;
  if (cmd === 'background_helpers') return BACKGROUND as unknown as T;
  // A helper's conversation once, with nothing new after it.
  if (cmd === 'background_transcript') {
    const first = args.since == null;
    return { ...HELPER_TRANSCRIPT, messages: first ? HELPER_TRANSCRIPT.messages : [] } as unknown as T;
  }
  if (cmd === 'background_at_stake') {
    return (args.ids as string[]).map(id => AT_STAKE[id] ?? { id }) as unknown as T;
  }
  if (cmd === 'applied_by_luna') return APPLIED as unknown as T;
  if (cmd === 'handoff_file') return HANDOFF_FILE as unknown as T;
  if (cmd === 'move_prepare') return MOVE_PREVIEW as unknown as T;
  if (cmd === 'handoff_transcript') {
    const first = args.since == null;
    return { ...HELPER_TRANSCRIPT, running: false, messages: first ? HELPER_TRANSCRIPT.messages : [] } as unknown as T;
  }
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
  // Settings' lists, so their shots show rows rather than empty states.
  if (cmd === 'codex_trusted_roots') return [String.raw`C:\src`, String.raw`D:\work\clients`] as unknown as T;
  if (cmd === 'browsers') return BROWSERS as unknown as T;
  if (cmd === 'settings_sync_state') return { claude: 'personal', codex: null, onStart: true } as unknown as T;
  return fallback;
}
