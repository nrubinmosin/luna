# Accounts and account files

## Responsibility

- List, create, rename and delete account folders.
- Read live usage limits for free (no tokens spent).
- Keep Claude Code's OAuth token fresh the way the CLI does.
- Write trust bits, recommended settings, synced settings and hand edits into
  files the CLIs also rewrite, without racing them.

## Key modules

| Path | What it holds |
| --- | --- |
| `accounts.rs` | folders under `<accounts root>/{anthropic,openai}/<name>`; a Codex account's seed `config.toml` |
| `settings.rs` | `<data>/settings.json` (accounts root, defaults, library reaches, `codexTrustedRoots`, `helpersAboveCaller`) |
| `claude/limits.rs`, `codex/limits.rs`, `throttle.rs` | usage windows and the shared 429 cool-off |
| `claude/oauth.rs` | access-token refresh |
| `claude/trust.rs`, `codex/trust.rs` | trust before the spawn |
| `checks.rs` | the Recommendations window's checks and fixes |
| `account_settings/` | recommended settings (`specs.rs`, `files.rs`, `edit.rs`), sync from a parent (`sync.rs`), the hand editor (`editor.rs`); `mod.rs` the lock every change goes under |
| `account_use.rs` | which accounts have a CLI of Luna's on them until it exits, and the holds that keep new ones off |
| `src/features/accounts/` | Accounts window, `RecommendedDialog`, `ConfigEditor`, `SettingsSync`, `LoginModal` |

## An account is a folder

`<accounts root>/anthropic/<name>` (Claude Code, `CLAUDE_CONFIG_DIR`) or
`<accounts root>/openai/<name>` (Codex, `CODEX_HOME`); default root
`Documents/luna-accounts`, kept in Rust settings. Create is mkdir, delete is a
recursive remove, list is a readdir of the two subfolders. A fresh Codex account
gets `check_for_update_on_startup = false`. A rename carries every chat, the
agents' list, the settings parent and all the library keeps of the account
along (docs/architecture/library.md, "Reaches"), refused while a chat or
helper runs on it, an idle or restored helper included
(`background::uses_account`) and a start waiting for its library pass
(`library::lay_out_for`), asked again under the library's lock. A rename and a
delete hold that lock throughout, and are refused without it.

## Limits

- **Claude Code:** the OAuth token from `<account>/.credentials.json`, then
  `api.anthropic.com/api/oauth/usage` (the endpoint `/usage` uses). The newer
  of the CLI's cache in `.claude.json` and Luna's own last answer answers while
  fresh (5 and 10 minutes), never the older: it could bring back a window the
  newer one lacks. Its `limits` rows by `kind`: `session` → 5 hours,
  `weekly_all` → week, each `weekly_scoped` → its model's week. A row of
  another kind is kept (`other`) under the server's own name for it (the
  scope's `display_name`, the `group`, or both) with its kind beside it,
  "monthly (monthly_all)", else the kind alone; its kind is logged once a run.
  The label is made of the row alone and is never a known window's: the quota
  notices know a window by its label, so it may not change with the rows
  around it.
- **Codex:** `tokens.access_token` / `account_id` from `<account>/auth.json`
  (email and plan from the `id_token` claims), then
  `chatgpt.com/backend-api/wham/usage` (what `/status` reads): `primary`,
  `secondary` and `additional_rate_limits`, labeled by length (300 min →
  "5 hours", 10080 → "week", any other spelled out, "1 day", logged once a
  run). Offline, throttled or with an expired token: the newer of Luna's own
  last answer and the last `token_count.rate_limits` of a rollout, never the
  older, for the same reason.
- **A window the provider did not send is absent, never 0%.** Claude's `h5`
  and `week` are `null` without their row, and each answer replaces the last
  whole, so a window the CLI's cache had and a newer answer lacks is gone; a
  Codex window that is `null` is left out, and an answer whose `rate_limit`
  holds no window at all is usage known (none), not a round that brought
  nothing, through which the UI keeps its last numbers. A row without its
  percent is no window either. The UI (`accounts.store.ts`) builds its rows from what is
  there, and agents get those rows as they are (`luna_account_list`, the quota
  notices, the account choice of a spawn given a list), so a missing window is
  no row anywhere. Seen 2026-11-09: Claude's endpoint sent no `session` row,
  and Codex's only the week.
- The page polls every 60 s; the row's ↻ asks at once, past the cache and the
  cool-off. A 429 backs off longer each time (`throttle.rs`, shared).

## Claude Code token refresh

An access token lives about 8 h; an account unopened for a while would hang on
"waiting for token refresh". `claude/oauth.rs` exchanges the refresh token
itself (`POST platform.claude.com/v1/oauth/token`, the CLI's client id and
scopes), by the CLI's rules, since refresh tokens rotate: both its mkdir locks
(`<config>/.oauth_refresh.lock` and `<config>.lock`, stale after 60 s), re-read
under the lock, write only if the refresh token on disk is still the one sent.
A CLI refreshing after Luna sees a new access token and takes it.
`invalid_grant` means a new login; the account shows signed out. The temp file
is created 0600.

Codex is left alone: its token lives 10 days, it refreshes itself, and the
server rejects a reused refresh token, so a refresh beside a live Codex would
sign it out.

## Trust

Neither CLI can show its trust prompt as Luna runs it (Claude Code refuses
under `--worktree`; Codex follows it with a Windows sandbox question), so the
bit is written before the spawn: `hasTrustDialogAccepted` in `.claude.json`, or
`[projects.'<native path>'] trust_level = "trusted"` in `config.toml` (via
`toml_edit`; the key compared case-insensitively and with either slash).

Codex can also be trusted for one run, `-c projects={"<checkout>"={trust_level=
"trusted"}}` (`trust_override`), which writes nothing: every background
helper's turn, and any session in a project under a folder of
`codexTrustedRoots` (Settings → Agents & trust → Trusted folders). Codex matches
trust by the exact path, never a parent's. With a Codex running on the account,
trust goes as this override instead of into `config.toml`.

Codex's "hooks unasked" (`codex/hooks.rs`): the default and per-folder choices
live in Luna's settings, where the core reads them for helpers too. A Codex
chat stopped on "Hooks need review" is an `ask` (`agents/ask.rs`
`codex_list_on`), the person's to answer.

## Recommendations (`checks.rs`)

What Luna's features rely on, as checks: Luna's settings, account files, the
machine's policy files. Each returns `status` (ok, off, blocked, unknown) with
`action`, `risk`, `note` and `refused`; only checks count on the status bar
button. `apply_recommendation(id, account, confirmed)` re-reads everything and
refuses a risky fix unconfirmed; nothing is applied unasked. A fix a sync could
undo also goes on the parent when the parent says the same, as one change. A
change that leaves the check off is an error. Codex's hooks feature is judged as
Codex resolves it, the account over the machine's `config.toml` (`/etc/codex`,
`%ProgramData%\OpenAI\Codex`).

## Recommended settings

Per account, key by key and platform-aware (`account_settings/specs.rs`). A tick
means "the file will have this key": present keys open ticked; unticking removes
a key only while it still holds the recommended value, and empty parents go too.
"Tick recommended" adds the `defaultOn` set; Apply shows `+N / −M`
(`apply_recommended(provider, account_path, add, remove)`). The row's button
shows `k/n`. Claude Code reads a few settings (`copyOnSelect`) only from
`.claude.json`: such a spec is `global`, and apply rewrites `.claude.json` only
for a key of its own.

## Shared settings (`account_settings/sync.rs`)

A parent account per CLI; its settings file is copied exactly onto the others
(now, or at every launch). Logins and Codex's trusted folders stay each
account's own. A blank Codex parent is valid. A sync waits for processes only,
not retained helpers.

## Hand editor

`ConfigEditor.tsx` is CodeMirror 6 (JSON via `@codemirror/lang-json`, TOML via
`@codemirror/legacy-modes`) with theme-token colors. The core checks as you type
(`check_account_config`, with line and column) and lays the file out on save
(`format_account_config`: JSON indented by two, TOML by the `taplo` crate,
comments and key order kept). Only what parses is written.

## Writing CLI files

- Every change goes under `editing()`, a file lock shared by window and core,
  taken before the first file is read; it fails closed (no lock, no write).
  Which accounts a fix writes is decided under it; `sync` takes the same lock.
- `.claude.json` and a Codex `config.toml` are rewritten by the CLI: they go
  through `account_use::quiet`, which spawns and helper turns wait out, and an
  account counts as `Running` until its process has exited, not until it leaves
  the session map.
- `settings.json` and `.claude.json` caught half-written or held open are read
  again; one that still will not parse is an error, never an empty object
  written back. `config.rs` does the same for TOML.
- An atomic rename keeps a file whole; it does not serialize concurrent updates,
  and a CLI running outside Luna is not excluded, which the UI says.

## Failure modes

| Failure | Behavior |
| --- | --- |
| Usage endpoint 429 | growing cool-off, shared by both providers |
| Refresh token rejected | account shown signed out |
| File unparseable | error shown; nothing written |
| CLI outside Luna writes concurrently | can race; documented in the row |
