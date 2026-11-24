# Accounts

## Account folders

An account is a folder under one root, by default `Documents/luna-accounts`,
split by vendor: `anthropic/<name>` for Claude Code, `openai/<name>` for Codex.
Everything about accounts is in the **Accounts** window, from the status bar
(click the accounts) or the title bar's Menu, as a window over the main one.
Each account shows its CLI, its plan or why it has none (signed out, limits
unavailable), its folder, and a bar for each of its usage windows with how
full it is and the clock time it resets. **Add account** asks which CLI and a
name, and **Create** makes the folder and opens its sign-in. **Delete...** asks
first, naming the folder it deletes and the chats still set to the account,
which have nowhere to start; if the folder there is no longer that one when you
say yes, nothing is deleted. **Skills & instructions** opens the library, and
**Settings** moves the root anywhere.

A Claude Code session starts with `CLAUDE_CONFIG_DIR=<account folder>`, a Codex
session with `CODEX_HOME=<account folder>`, so logins, settings and history are
per account. Nothing is written into a fresh account: what a CLI needs inside
Luna is passed at every start ([What Luna passes at every start](#what-luna-passes-at-every-start)).

- **Sign in** with the button on a fresh account's row: it opens the CLI's
  login, a bare `claude` or `codex login` run in the account's folder, in a
  terminal over the Accounts window. A new account opens it by itself, and only
  on the folder Luna just made: if the accounts folder was moved or the
  account's folder replaced meanwhile, it says so and starts nothing. It closes
  by itself when the CLI exits, and the account's row is read again; if the CLI
  exits with an error, the terminal stays so that what it said can be read,
  until you close it (its X, or Esc when the terminal has not been clicked: in
  the terminal Esc is the CLI's). Closing it ends the session. A Claude Code account whose login has run out
  (the server turned its refresh token down, and Luna or the CLI cleared its
  tokens) shows signed out with the same button, and helpers
  no longer go to it. Luna never renews a Codex login, and
  Codex leaves `auth.json` as it is when its own renewal is refused, so a
  Codex account shows signed out only once its tokens are gone or blank.
- **Rename** on the account's row: the folder is renamed and every
  chat on it, the agents' list, the settings parent and what Skills &
  instructions knows of its copies follow. Not while a chat
  or background helper on it is running. An account's name cannot end in a dot
  or a space.
- **Agents' use:** every account is available to agents unless switched off:
  untick "Agents may spawn sessions on this account", on the line of its row
  that names the folder, and the box then reads "Agents may not use this
  account: tick to allow". The switch is read as the window opens and
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
- A 429 backs off longer each time; **Refresh** on the row asks again past the
  cache where a request can be made, and for Claude Code tries to renew an
  expired token (a Codex login's renewal is the CLI's; an expired one shows
  the last known limits). The limits are read as the window
  opens and on Refresh; what a Refresh or a sign-in reads shows in the status
  bar, and reaches agents' `luna_account_list`, at once.
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
afterwards. The one exception is a setting you choose in Luna that no start can
pass: Claude Code's copy-on-select (below).

- **Claude Code chats:** `tui: fullscreen`, in the settings file Luna passes with
  `--settings`, and `CLAUDE_CODE_NO_FLICKER=1`. The renderer draws on the
  alternate screen with its own scrollback, and the variable holds it even after
  the CLI gave up on it once (`fullscreenAutoDisabled` in `.claude.json`, which
  the variable outranks). `--system-prompt-snapshot off` renders the system
  prompt fresh so a resumed chat uses the instructions Luna passes at that start.
- **Copy on select**, in both CLIs: whether selecting text in a chat copies
  it at once. Settings › Chats › Copy on select: **Off** (the default, as
  Luna's terminal has it), **On**, or **As the account has it**. A Codex chat
  is told at every start: `-c tui.copy_on_select="never"` for Off, `"always"`
  for On, and nothing for As the account has it, so its `config.toml` (or
  Codex's own default, `auto`) decides; nothing is written into the account.
  Claude Code reads it from the account's `.claude.json` alone, so before each
  Claude Code chat starts, Luna sets `copyOnSelect` there when it differs from
  your choice, under Claude Code's own lock on that file. No other key's value
  changes, though the file is written out again (pretty-printed, keys in their
  order). It is set before the CLI starts, so a start that then fails may
  still leave it changed. As the account has it
  leaves the file alone, and `/config` in Claude Code sets it. An account with
  no `.claude.json` yet gets none from Luna: Claude Code makes it as it first
  starts, and the next chat sets the key. If the file cannot be written (Claude
  Code is rewriting it, or it does not read as JSON), the chat starts all the
  same and Luna's log says why; it notes each write too. A `.claude.json`
  that is a link when Luna looks, just before the write, is not written
  through. Sign-ins and helpers
  are left alone.
- **Codex chats:** `-c check_for_update_on_startup=false` (Luna updates the CLI
  itself) and `-c tui.notifications=false` (Luna says when a chat is waiting),
  plus `--no-daemon` to run the chat in its own process and
  `-c tui.terminal_title=["thread-id"]` so Luna can pick up the thread ID.
  When the local listener and curl 8.3 or later are available, Luna passes `-c hooks.<event>`
  for its status hooks and `-c hooks.state` to trust those hooks for that run.
- **Codex on Windows:** chats and helpers in `read-only`, `read-only-net`,
  `workspace-write`, or Codex's own mode get `-c windows.sandbox="unelevated"`
  when their account config leaves `windows.sandbox` unset.
- **Helpers:** Codex's app-server gets the update-check override as well.
  Claude Code helpers have no fullscreen UI. Luna strips any inherited
  `CLAUDE_CODE_NO_FLICKER` from their environment.

Luna's earlier versions could write such settings into an account from a
Recommendations window. What they wrote stays where it is: delete a key from the
account's `settings.json`, `.claude.json` or `config.toml` yourself if you no
longer want it. Account files open in any editor; Luna has none of its own.

## Shared settings

Pick a parent account per CLI and its settings file is copied onto the others:
now, with **Sync now**, or every time Luna starts with **Copy from the parents
every time Luna starts** ticked. The others get an
exact copy: a key taken out of the parent goes from them too, and whatever was
set on a child alone is replaced. Logins, Codex's trusted folders and the
hooks you trusted in Codex ("Hooks need review", its `hooks.state`) stay each
account's own: the hooks themselves are copied, never the parent's trust in
them.

Not in the Accounts window, as [ADR-0030](../adr/0030-scope-of-0-20.md) has it: the
recommended settings and recommendations, the settings file editor (edit an
account's files in an editor of your own) and what Luna applied.
