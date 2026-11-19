# Settings

The gear at the right of the status bar opens Settings; so does the gear in the
Accounts window, on the General page. Pages are listed on the left and the page
itself is on the right. Only the page scrolls: the title bar, the list and the
page’s title stay where they are. The dialog is 920 px wide and as tall as its
page, up to 720 px, or the window less a margin where that is smaller; its top
edge stays put as pages change. It reopens on the page it was left on. Escape
or a click beside it closes it. Something typed and not yet saved on one page
(the waiting marker's text) is still there after a look at another, until
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
| General | Board, Notifications, Waiting for you, Accounts | Auto-tile the board; play a sound; the waiting marker and its text; the accounts folder |
| Chats | New chats start with, Worktrees | Git worktree, Luna tools, Codex hooks without asking and Wait guard, as the new-chat dialog starts them; fetch origin before Claude Code makes a worktree; delete worktrees by default |
| Agents & trust | Helpers, Trusted folders | Allow helpers more permissions than their parent chat; helpers per chat (the places a chat's line of helpers shares, 1 to 32, 8 by default); helpers' mode when not given (the spawning chat's, the default, or the account's); auto-handoff for leads and its threshold (on, at 60% of a lead's context, by default: [agents](agents.md#handoff)); the folders whose projects Codex sessions start trusted |
| CLIs | Claude Code, Codex | Each one's version and Check now; each installed version tagged current, previous, in use or turned away (one below the oldest this Luna runs says so), and the versions Luna turned away, with why: rolled back from after its first sessions failed, or failed the offline check. Nothing here chooses a version: Luna keeps the newest and rolls back on its own ([updates](updates.md#the-clis)) |
| Updates | — | Luna and the core: each version, its state and its button |

- **New chats start with:** how the new-chat dialog's boxes start, and how
  Ctrl+Shift+N makes a chat, in a folder with no start of its own ("keep for
  this folder" in the dialog). Wait guard is off unless you turn it on; on
  Codex it applies only with hooks without asking. Helpers have the guard
  whatever it says.
- **Waiting marker:** the checkbox turns it on and off; the text under it is
  saved with Save or Enter.
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

Release 0.20's native window has Settings as a window over the main one, with
the same list of pages and less on them
([ADR-0030](../adr/0030-scope-of-0-20.md)):

| Page | Settings |
| --- | --- |
| General | Play a sound; the accounts folder (Change…, Default) |
| Chats | Git worktree (offered by the new-chat dialog only in a folder in git) and Luna tools, as the new-chat dialog starts them |
| Agents & trust | Allow helpers more permissions than their parent chat (turning it on asks first, in the window); helpers per chat (- and +); helpers' mode when not given; when a helper's context is told; trusted folders (Add folder…, Remove) |
| CLIs | For each CLI its version, what its last check found, and Check; a version the automatic rollback turned away says so. Nothing here chooses a version ([updates](updates.md#the-clis)) |
| Updates | Luna's version and Check; Update… asks with the new version and its notes before it downloads and installs; the core's version, since when it runs, and Restart |

Gone: auto-tile, the waiting marker, Codex hooks without asking, the wait
guard, the worktree fetch and delete defaults, auto-handoff for leads, the
CLIs' channels, Use, marks, known-good list and Keep it. Long work says where
it is in words (Checking…, Downloading 0.171.0: 50%), and a time is the clock
time it happened. The sound and the new chat's boxes are kept in
`window.json` in Luna's data folder.
