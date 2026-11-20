# Accounts and account files

## Responsibility

- List, create, rename and delete account folders.
- Read live usage limits for free (no tokens spent).
- Keep Claude Code's OAuth token fresh the way the CLI does.
- Write trust bits and synced settings into files the CLIs also rewrite,
  without racing them. Nothing else of Luna's goes into an account's own files:
  what a CLI needs inside Luna is passed at every start.

## Key modules

| Path | What it holds |
| --- | --- |
| `accounts.rs` | folders under `<accounts root>/{anthropic,openai}/<name>`; nothing written into a new one |
| `settings.rs` | `<data>/settings.json` (accounts root, defaults, library reaches, `codexTrustedRoots`, helpers' places and context share) |
| `claude/limits.rs`, `codex/limits.rs`, `throttle.rs` | usage windows and the shared 429 cool-off |
| `claude/oauth.rs` | access-token refresh |
| `claude/trust.rs`, `codex/trust.rs` | trust before the spawn |
| `account_settings/` | sync from a parent (`sync.rs`), the file found, checked and written (`files.rs`); `mod.rs` the lock every change goes under |
| `account_use.rs` | which accounts have a CLI of Luna's on them until it exits, and the holds that keep new ones off |
| `src-tauri/crates/window/src/accounts/` | Accounts panel, shared settings and sign-in terminal |

## An account is a folder

`<accounts root>/anthropic/<name>` (Claude Code, `CLAUDE_CONFIG_DIR`) or
`<accounts root>/openai/<name>` (Codex, `CODEX_HOME`); default root
`Documents/luna-accounts`, kept in Rust settings. A deletion holds the
folder to `boundary::account_in` first (neither the provider's folder nor
the account's a link or junction, and inside the root once resolved), and
the native window's is bound to the folder its question named, by path and
identity on the drive (`account_identity`, `delete_account_checked`).
Create is mkdir; delete opens the provider's folder and the account's as
themselves (no link, junction or other reparse point; on Unix on the
accounts root's device), holds the account's to the identity asked about
where there is one, and removes it through that handle
(`held::Dir::remove_tree`): a link inside goes as itself, a mount inside
refuses the removal before anything goes. List is a readdir of the two
subfolders. A fresh account gets no file. A rename carries every chat, the
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
  percent is no window either. The window's usage model builds its rows from what is
  there, and agents get those rows as they are (`luna_account_list`, the quota
  notices, the account choice of a spawn given a list), so a missing window is
  no row anywhere. Seen 2026-11-09: Claude's endpoint sent no `session` row,
  and Codex's only the week.
- The window polls every 60 s; the row's ↻ asks at once, past the cache and the
  cool-off. A 429 backs off longer each time (`throttle.rs`, shared).

## Claude Code token refresh

An access token lives about 8 h; an account unopened for a while would hang on
"waiting for token refresh". `claude/oauth.rs` exchanges the refresh token
itself (`POST platform.claude.com/v1/oauth/token`, the CLI's client id and
scopes), by the CLI's rules, since refresh tokens rotate: both its mkdir locks
(`<config>/.oauth_refresh.lock` and `<config>.lock`, stale after 60 s), re-read
under the lock, write only if the refresh token on disk is still the one sent.
A CLI refreshing after Luna sees a new access token and takes it.
The answer is judged as the CLI judges it (`refusal`): a 400 or 401 saying
`invalid_grant` (a string `error`, or an object's `type`) means a new login;
one marked `account_on_hold` is not, since a login does not lift a hold, and
every other answer is a failed refresh, tried again after 5 minutes. The CLI
reads the hold only from the top-level `error_description`; Luna also from an
object `error`'s `message`, since a sign-out cannot be taken back. The temp
file is created 0600. The locks get no heartbeat from Luna, so the exchange
has a deadline of 20 s, and a refusal counts only if it came back within 30 s
of reaching for the locks, half their stale window: later, a CLI may have
taken them as stale and rotated the token, and the refusal is a failed
refresh, tried again.

On `invalid_grant` Luna signs the account out the way the CLI does
(`sign_out`): still under both locks it re-reads the file and, only if it
still holds the refresh token that was refused, blanks the tokens
(`refreshToken: ""`, `accessToken: ""`, `expiresAt: 0`; `loginEnded:
"time_limit"` when the top-level `error_description` says the token expired,
else no `loginEnded`), keeping every other field. It goes through the private
temp file and a rename only: where a renewal would fall back to writing in
place, a sign-out fails and leaves the file as it was. The file is then the
only record of the refusal: the panel (window process), the core's account
choice and move check, and any later start all read it as signed out. The
process that got the refusal shows "sign-in expired — log in again"; after a
restart the row says "No usable credentials — sign in", like any blank file
(`loginEnded` is not read). A file holding another token by then was written
by a login since, and is left alone. A file that cannot be replaced keeps
saying signed in: the process that got the refusal remembers the token in
memory, posts it no more, and its row says the sign-in expired and
`.credentials.json` could not be updated. Only then does the core not know,
and may still send a helper to the account, until a restart
posts the token once more.

Signed in means a refresh token that is not blank, not past its
`refreshTokenExpiresAt` (`oauth::signed_in`): the panel's row and the choice of
an account for a helper (`agents/spawn/account.rs`
`signed_in`) both go by it, and `scripts/lib.ps1` `Test-AccountSignedIn` by
the same rule. On `invalid_grant` the CLI blanks its tokens, and Luna its
own refused ones (above), so that file reads as signed out, offers Sign in,
and no refresh is posted with it. A Codex
`auth.json` is signed in by the same measure: both of its ChatGPT tokens there
and not blank (`codex::limits::chatgpt_login`). The exception is
`auth_mode: "chatgptAuthTokens"`, tokens a host application gives Codex, with
an empty refresh token Codex never uses: there a non-blank access token is the
login. Codex 0.169.0 keeps such tokens in memory and writes no `auth.json` for
them, but its loader would take one, so Luna reads one the same way.

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

A Codex chat stopped on "Hooks need review" is an `ask` (`agents/ask.rs`
`codex_list_on`), the person's to answer: Luna never passes
`--dangerously-bypass-hook-trust`.

## What a start passes instead of writing

What a CLI needs inside Luna never goes into an account's files. Each start
passes it, and `Item`s in `applied.rs` name it for the goldens (`golden.rs`
`assert_shown`):

- Claude Code chat or login: `tui: "fullscreen"` in the `--settings` file or inline
  JSON (`claude_settings`), and `CLAUDE_CODE_NO_FLICKER=1`
  (`claude/launch.rs` `FULLSCREEN`, `NO_FLICKER_ENV`). Its copy-on-select is read
  from the global config (`.claude.json`) only, which no start can reach: not set.
- Codex chat: `-c check_for_update_on_startup=false`, `-c tui.notifications=false`,
  `-c tui.copy_on_select="never"` (`codex/launch.rs` `NO_UPDATE_CHECK`,
  `TUI_SETTINGS`). A helper's `app-server` has no TUI: it keeps the update check alone.

Luna 0.19 wrote such keys into accounts from its Recommendations window and
noted trust writes in `<data>/applied-ledger.json`. 0.20 removes the ledger
file at the window's start (`applied::drop_0_19_ledger`) and leaves what was
written into accounts where it is: an account's files are not Luna's to clean.
The settings keys `codexHooksUnasked`, `codexHooksUnaskedIn` and
`recommendationsDismissed` are passed over when `settings.json` is read and gone
at its next write.

## Shared settings (`account_settings/sync.rs`)

A parent account per CLI; its settings file is copied exactly onto the others
(now, or at every launch). Logins and Codex's trusted folders stay each
account's own. A blank Codex parent is valid. A sync waits for processes only,
not retained helpers.

## Writing CLI files

- Every change goes under `editing()`, a file lock shared by window and core,
  taken before the first file is read; it fails closed (no lock, no write).
  `sync` and the trust writes take it.
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
| Refresh token rejected (400/401 `invalid_grant`) | account shown signed out |
| Refresh token blanked by the CLI | account shown signed out; no refresh sent |
| Token endpoint answers anything else | "token refresh failed"; retried after 5 minutes |
| File unparseable | error shown; nothing written |
| CLI outside Luna writes concurrently | can race; documented in the row |
