# Settings

Settings is a window over the main one, opened by the status bar's Settings
button or the Accounts window's. It opens on the General page every time.
Pages are listed on the left, with Luna's version under them, and the page is
on the right; only the page scrolls. Esc closes a question Settings is asking
(Roll back's), then Settings itself; so does its X. It has less on its pages
than 0.19's ([ADR-0030](../adr/0030-scope-of-0-20.md)).

Each setting is a row: its name and a line on what it does on the left, its
control on the right.

## Pages

| Page | Settings |
| --- | --- |
| General | Window scale (80% to 250%, on top of the display's scaling and Windows' text size: [the window](window.md)); play a sound; ask to turn G-SYNC off in NVIDIA's profile that holds Luna's exe name (Windows with an NVIDIA driver: [the window](window.md), G-SYNC); the accounts folder (Change…, Default) |
| Chats | Git worktree (offered by the new-chat dialog only in a folder in git) and Luna tools, as the new-chat dialog starts them; copy on select, for Claude Code and Codex chats (Off, On, As the account has it) |
| Agents & trust | Helpers per chat (- and +, 16 to start); **Tell me when a helper's context passes** (Never, or 30% to 90%; 50% to start); trusted folders (Add folder..., Remove) |
| CLIs | For each CLI its version, what its last check found, and Check; **Roll back to** the version kept before this one, shut with the reason beside it where it would be refused; a version the automatic rollback turned away says so, and an open Settings shows it as soon as the rollback is made. Nothing here chooses a version otherwise ([updates](updates.md#the-clis)) |
| Updates | Luna's version, when it was last checked (a clock time, while it is the newest) and Check; Update... opens the status bar's update confirmation, the one the update chip opens, with the new version and its notes, before it downloads and installs; the core's version and the first 8 characters of its fingerprint, since when it runs and its process id, and Restart (Start when no core runs; off while an update holds the core), which ends the sessions in it: the chats resume in the new core from their transcripts |

- **New chats start with:** how the new-chat dialog's boxes start in a folder
  with no start of its own; a flip in the dialog is for that chat alone.
- **Accounts folder:** Change... and Default move where Luna looks for accounts.
  The accounts themselves are moved by hand.
- **Copy on select** (both CLIs): Off by default. A Codex chat is told at
  every start (`never` or `always`); Off and On are written into each Claude
  Code account's `.claude.json` before its next chat starts, where it differs.
  As the account has it passes nothing to Codex and leaves Claude Code's file
  alone ([accounts](accounts.md#what-luna-passes-at-every-start)). It is kept in
  `settings.json` in Luna's data folder, as `copyOnSelect`.

Gone: the cap on helpers' privilege and a helper's default mode,
the waiting marker, Codex hooks without asking, the wait guard, the worktree fetch and delete defaults, auto-handoff for leads, the
CLIs' channels, Use, marks, known-good list and Keep it. Long work says where
it is in words (Checking..., Downloading 0.171.0: 50%), and a time is the clock
time it happened. Auto-tile is on the title bar ([the window](window.md)). The sound and the new chat's boxes are kept in
`window.json` in Luna's data folder; the window scale is kept with the chat
list (`window\chats.json`), and Ctrl+=, Ctrl+- and Ctrl+0 change it too.

**Roll back** asks once, naming the CLI and both versions ("Roll back Claude
Code from 2.1.330 to 2.1.328?"), and then says what it did ("Rolled back Claude
Code: 2.1.330 → 2.1.328.") or why it did nothing, in the core's words. If the
current version is no longer the one you saw (a check installed another), it
says so and the versions on the page are read again; look and press it again.
Running sessions keep their version ([updates](updates.md#the-clis)).
