# Accounts

## Account folders

An account is a folder under one root, by default `Documents/luna-accounts`,
split by vendor: `anthropic/<name>` for Claude Code, `openai/<name>` for Codex.
Everything about accounts is in the **Accounts** window, from the status bar:
"Add account" asks which CLI and creates the folder, the bin deletes it after a
confirmation, Settings moves the root anywhere.

A Claude Code session starts with `CLAUDE_CONFIG_DIR=<account folder>`, a Codex
session with `CODEX_HOME=<account folder>`, so logins, settings and history are
per account. Nothing is written into a fresh account: what a CLI needs inside
Luna is passed at every start ([What Luna passes at every start](#what-luna-passes-at-every-start)).

- **Sign in** with the button on a fresh account's row: it runs `claude`, or
  `codex login`, in a modal. A Claude Code account whose login has run out
  (the server turned its refresh token down, and Luna or the CLI cleared its
  tokens) shows signed out with the same button, and helpers
  no longer go to it. Luna never renews a Codex login, and
  Codex leaves `auth.json` as it is when its own renewal is refused, so a
  Codex account shows signed out only once its tokens are gone or blank.
- **Rename** with the pencil beside the name: the folder is renamed and every
  chat on it, the agents' list, the settings parent and what Skills &
  instructions knows of its copies follow. Not while a chat
  or background helper on it is running.
- **Agents' use:** every account is available to agents unless switched off:
  untick "Agents may spawn sessions on this account", on the line of its row
  that names the folder (the WebView window has the robot button on the row for
  it), and the box then reads "Agents may not use this account". The switch is read as the window opens and
  on Refresh, and is refused while an update is being installed, like any other
  change to an account.

## Live limits

Limits cost no tokens.

- Each account shows the windows its provider reports, each with its reset (in
  so long, and the exact time in the Accounts window). A window the provider
  does not send is left out, not shown at 0%, and one Luna does not know yet is
  shown under the provider's own name for it.
- **Claude Code:** the 5-hour session window and the week across all models
  when the usage endpoint sends them, and every per-model weekly limit, from the
  OAuth usage endpoint `/usage` uses, the CLI's own cache answering most rounds.
- **Codex:** the ChatGPT plan's rate-limit windows, each named by its length
  ("5 hours", "week", "1 day"), plus per-model limits, from the endpoint Codex's
  `/status` reads, with the token in `auth.json`, asked at most every 10
  minutes; in between, and when unreachable, Luna's last answer or the last
  `token_count` of a rollout, whichever is newer.
- A 429 backs off longer each time; the refresh button on the row asks now.
- A Claude Code access token lasts about eight hours; for an account nobody has
  opened since, Luna renews it the way the CLI does, under the CLI's own lock, so
  a session running beside it picks up the new token.

## Trust

Luna writes the CLI's own trust bit for a folder before a session starts:
`hasTrustDialogAccepted` in `.claude.json`, or `[projects.'<path>'] trust_level
= "trusted"` in `config.toml`. Claude Code cannot show its trust prompt under
`--worktree`, and Codex follows its trust screen with a Windows sandbox
question. Codex can also be trusted for one run (`-c projects=…`), which writes
nothing: background Codex helpers are trusted that way wherever they are sent,
and so is any Codex session under a folder in Settings → Agents & trust →
Trusted folders. Codex matches trust by the exact path, so a parent folder in its
config would not do.

## What Luna passes at every start

Luna writes no setting into an account for its own sake. What a CLI needs to
work well inside Luna goes with each start, on the command line and in the
environment, so the account's files stay yours and nothing has to be put right
afterwards.

- **Claude Code chats:** `tui: fullscreen`, in the settings file Luna passes with
  `--settings`, and `CLAUDE_CODE_NO_FLICKER=1`. The renderer draws on the
  alternate screen with its own scrollback, and the variable holds it even after
  the CLI gave up on it once (`fullscreenAutoDisabled` in `.claude.json`, which
  the variable outranks). Luna does not set Claude Code's copy-on-select: 2.1.325
  reads it from `.claude.json` alone, which Luna must not write and no start can
  reach, so it stays as the account has it (on unless you turned it off in the
  CLI).
- **Codex chats:** `-c check_for_update_on_startup=false` (Luna updates the CLI
  itself), `-c tui.notifications=false` (Luna says when a chat is waiting) and
  `-c tui.copy_on_select="never"`.
- **Helpers:** they have no screen, so nothing of the above but Codex's update
  check, which every start has. Luna also takes `CLAUDE_CODE_NO_FLICKER` out of
  what a helper inherits, so a Luna started from a fullscreen chat's shell does
  not pass it on.

Luna's earlier versions could write such settings into an account from a
Recommendations window. What they wrote stays where it is: delete a key from the
account's `settings.json`, `.claude.json` or `config.toml` yourself if you no
longer want it. Account files open in any editor; Luna has none of its own.

## Shared settings

Pick a parent account per CLI and its settings file is copied onto the others:
now, with Sync now, or at every launch with the box ticked. The others get an
exact copy: a key taken out of the parent goes from them too, and whatever was
set on a child alone is replaced. Logins, Codex's trusted folders and the
hooks you trusted in Codex ("Hooks need review", its `hooks.state`) stay each
account's own: the hooks themselves are copied, never the parent's trust in
them.

## In the native window

Release 0.20's native window has Accounts as a window over the main one:
each account with its CLI, its plan or why it has none (signed out, limits
unavailable), its folder, and a bar for each of its usage windows with how
full it is and the clock time it resets; Rename, Refresh (past the CLI's
cache, renewing an expired token), Sign in (on a signed-out account, below)
and Delete…, which asks first, naming the folder it deletes and the chats
still set to the account, which have nowhere to start; if the folder there is
no longer that one when you say yes, nothing is deleted; Add account
with its CLI and name; Shared settings, each CLI's parent account with Sync
now and the box for copying at every start; and buttons to the library and
Settings. The limits are read as the window opens and on Refresh.

Rename also renames the account in every chat set to it.

**Sign in** opens the CLI's login in a terminal over the Accounts window: a
bare `claude`, or `codex login`, run in the account's folder. It closes by
itself when the CLI exits, and the account's row is read again; if the CLI
exits with an error, the window stays so that what it said can be read, until
you close it (its X, or Esc when the terminal has not been clicked: in the
terminal Esc is the CLI's). Closing it ends the session. The window
opens from the title bar's Menu, Accounts.

Not in it, as [ADR-0030](../adr/0030-scope-of-0-20.md) has it: the
recommended settings and recommendations, the settings file editor (edit an
account's files in an editor of your own) and what Luna applied.
