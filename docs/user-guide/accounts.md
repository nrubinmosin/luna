# Accounts

## Account folders

An account is a folder under one root, by default `Documents/luna-accounts`,
split by vendor: `anthropic/<name>` for Claude Code, `openai/<name>` for Codex.
Everything about accounts is in the **Accounts** window, from the status bar:
"Add account" asks which CLI and creates the folder, the bin deletes it after a
confirmation, Settings moves the root anywhere.

A Claude Code session starts with `CLAUDE_CONFIG_DIR=<account folder>`, a Codex
session with `CODEX_HOME=<account folder>`, so logins, settings and history are
per account. A fresh Codex account gets a `config.toml` with
`check_for_update_on_startup = false`: Luna updates that CLI itself.

- **Sign in** with the button on a fresh account's row: it runs `claude`, or
  `codex login`, in a modal.
- **Rename** with the pencil beside the name: the folder is renamed and every
  chat on it, the agents' list, the settings parent and what Skills &
  instructions knows of its copies follow. Not while a chat
  or background helper on it is running.
- **Agents' use:** every account is available to agents unless switched off with
  the robot button on its row.

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
  `/status` reads, with the token in `auth.json`; when unreachable, the last
  `token_count` of a rollout.
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

## Recommendations

One window from a button always in the status bar, which counts what is open.

- **What Luna's features rely on**, in Luna's settings and the accounts' files:
  Codex hooks unasked (the guard reaches Codex only with it), `disableAllHooks` or
  Codex's hooks feature off on an account, Bypass not accepted, a fullscreen
  renderer Claude Code gave up on, keep awake, the waiting marker, fetching
  before a worktree. Each says what it is for and is fixed with one click, never
  by itself; "Leave it" keeps a deliberate choice from nagging. A fix that does
  more than its title says (bringing back every hook of an account, reaching
  other accounts through a settings parent) says so and asks first. An
  administrator's policy shows as Blocked, with no button.
- A Codex file, and Claude Code's `.claude.json`, are written only while no
  session of Luna's is on the account, until its process has exited; a CLI
  running outside Luna can still race that write.

## Recommended settings

Per account, under Recommendations and on the account's row. The button says how
many the file has (`5/12`): what works well for the CLI inside Luna, each with
what the file says now, what it would say and why.

- **Claude Code:** `tui: fullscreen`, no spinner tips or feedback prompts, no
  compacting while idle, no second Bypass confirmation, PowerShell for `!`
  commands (Windows only), no copy on select (kept in `.claude.json`).
- **Codex:** no update check, tooltips, desktop notifications, copy on select,
  analytics or feedback prompts, live web search.
- As matters of taste: attribution, Bypass by default.

Nothing is written until asked. A tick is what the file will have: keys already
there open ticked, and unticking one takes it out (only while it still says what
was recommended). "Tick recommended" ticks the everyday set; Apply says how many
keys go in and out. The rest of the file, comments included, stays.

## Editing a settings file

`settings.json` (Claude Code) or `config.toml` (Codex) opens in an editor with
syntax colors and line numbers. It is checked as you type (the breaking line is
marked), written only once it parses, and laid out on save the way the CLI
writes it (JSON indented by two, TOML by taplo's rules), comments and key order
kept. Ctrl+S saves.

## Shared settings

Pick a parent account per CLI and its settings file is copied onto the others:
now, with Sync now, or at every launch with the box ticked. The others get an
exact copy: a key taken out of the parent goes from them too, and whatever was
set on a child alone is replaced. Logins and Codex's trusted folders stay each
account's own.
