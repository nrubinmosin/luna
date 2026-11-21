# Settings

The gear at the right of the status bar opens Settings; so does the gear in the
Accounts window, on the General page. Pages are listed on the left and the page
itself is on the right. Only the page scrolls: the title bar, the list and the
page’s title stay where they are. The dialog is 920 px wide and as tall as its
page, up to 720 px, or the window less a margin where that is smaller; its top
edge stays put as pages change. It reopens on the page it was left on. Escape
or a click beside it closes it. Something typed and not yet saved on one page
is still there after a look at another, until
Settings closes.

Each setting is a row: its name and one line on what it does on the left, its
control on the right. A list (trusted folders, the CLIs' versions) sits under
its row, with its own buttons (Add folder…) under it on the left; each list
says so when it is empty. What a change costs stays in view under its row,
marked with a warning sign where it loses work or lets an agent do more. The
rest of each group's explanation is folded into a "How … works" line at the
foot of its card.

## Pages

| Page | Groups | Settings |
| --- | --- | --- |
| General | Board, Notifications, Accounts | Auto-tile the board; play a sound; the accounts folder |
| CLIs | Claude Code, Codex | Each one's version and Check now (and, in the new window, Roll back to the version before: [updates](updates.md#the-clis)); each installed version tagged current, previous, in use or turned away (one below the oldest this Luna runs says so), and the versions Luna turned away, with why: rolled back from after its first sessions failed, or failed the offline check. Nothing here chooses a version: Luna keeps the newest and rolls back on its own ([updates](updates.md#the-clis)) |
| Chats | New chats start with | Git worktree (a flag for the CLI, offered only in a git repository) and Luna tools, as the new-chat dialog starts them |
| Agents & trust | Helpers, Trusted folders | Helpers per chat (the places a chat's line of helpers shares, taken only by those at work: 1 to 64, 16 by default); when a helper's notices say how full its context is; the folders whose projects Codex sessions start trusted |
| Updates | — | Luna and the core: each version, its state and its button |

- **New chats start with:** how the new-chat dialog's boxes start, and how
  Ctrl+Shift+N makes a chat, in a folder with no start of its own ("keep for
  this folder" in the dialog).
- **Accounts folder:** Change… and Default move where Luna looks for accounts.
  The accounts themselves are moved by hand.

## Keyboard

- Settings opens with the focus on its page list. Up and Down move to the
  previous and next page and show it, Home and End to the first and last.
- Tab goes from the list into the page and through its rows in order; a row
  scrolled out of view comes into view as it takes the focus. Space toggles a
  checkbox, and Enter or Space opens a "How … works" line.
- Tab and Shift+Tab stay inside Settings, going round from its last control to
  its Close button; the rest of the window cannot be reached while it is open.
  On closing, the focus goes back to what opened it.
- In the Add an exception… menu, Up and Down move between entries, Home and End
  to the first and last, Enter picks one, and Escape closes the menu without
  closing Settings. Tab closes it and goes on to the next control, Shift+Tab
  back to the button.

## In the native window

Release 0.20's native window has Settings as a window over the main one (the
title bar's Menu, Settings, until the status bar has its gear), with the same
list of pages and less on them ([ADR-0030](../adr/0030-scope-of-0-20.md)):

| Page | Settings |
| --- | --- |
| General | Window scale (80% to 250%, on top of the display's scaling and Windows' text size: [the window](window.md)); play a sound; the accounts folder (Change…, Default) |
| Chats | Git worktree (offered by the new-chat dialog only in a folder in git) and Luna tools, as the new-chat dialog starts them |
| Agents & trust | Helpers per chat (- and +); when a helper's context is told; trusted folders (Add folder…, Remove) |
| CLIs | For each CLI its version, what its last check found, and Check; **Roll back to** the version kept before this one, shut with the reason beside it where it would be refused; a version the automatic rollback turned away says so. Nothing here chooses a version otherwise ([updates](updates.md#the-clis)) |
| Updates | Luna's version and Check; Update… opens the status bar's update confirmation, the one the update chip opens, with the new version and its notes, before it downloads and installs; the core's version, since when it runs, and Restart |

Gone: the cap on helpers' privilege and a helper's default mode,
auto-tile, the waiting marker, Codex hooks without asking, the wait guard, the worktree fetch and delete defaults, auto-handoff for leads, the
CLIs' channels, Use, marks, known-good list and Keep it. Long work says where
it is in words (Checking…, Downloading 0.171.0: 50%), and a time is the clock
time it happened. The sound and the new chat's boxes are kept in
`window.json` in Luna's data folder; the window scale is kept with the chat
list (`window\chats.json`), and Ctrl+=, Ctrl+- and Ctrl+0 change it too.

**Roll back** asks once, naming the CLI and both versions ("Roll back Claude
Code from 2.1.330 to 2.1.328?"), and then says what it did ("Rolled back Claude
Code: 2.1.330 → 2.1.328.") or why it did nothing, in the core's words. If the
current version is no longer the one you saw (a check installed another), it
says so and the versions on the page are read again; look and press it again.
Running sessions keep their version ([updates](updates.md#the-clis)).
