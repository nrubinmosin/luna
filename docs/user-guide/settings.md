# Settings

The gear at the right of the status bar opens Settings; so does the gear in the
Accounts window, on the General page. Pages are listed on the left and the page
itself is on the right. Only the page scrolls: the title bar, the list and the
page’s title stay where they are. The dialog is 920 px wide and as tall as its
page, up to 720 px, or the window less a margin where that is smaller; its top
edge stays put as pages change. It reopens on the page it was left on. Escape
or a click beside it closes it. Something typed and not yet saved on one page
(the waiting marker's text, a browser of your own) is still there after a look
at another, until Settings closes.

Each setting is a row: its name and one line on what it does on the left, its
control on the right. A list (trusted folders, link exceptions, browsers) sits
under its row, with its own buttons (Add folder…, Add an exception…, Add a
browser) under it on the left; each list says so when it is empty. What a
change costs stays in view under its row, marked with a warning sign where it
loses work or lets an agent do more. The rest of each group's explanation is
folded into a "How … works" line at the foot of its card.

## Pages

| Page | Groups | Settings |
| --- | --- | --- |
| General | Board, Notifications, Waiting for you, Accounts | Auto-tile the board; play a sound; the waiting marker and its text; the accounts folder |
| Chats | New chats start with, Worktrees | Git worktree, Luna tools and Codex hooks without asking, as the new-chat dialog starts them; fetch origin before Claude Code makes a worktree; delete worktrees by default |
| Agents & trust | Helpers, Trusted folders | Allow helpers more permissions than their parent chat; the folders whose projects Codex sessions start trusted |
| Links | Open links in, Browsers | The default browser and the accounts and project folders that take another; the browsers found on this computer and your own |
| CLIs | Claude Code, Codex | The channel each follows (Known-good or Latest); its version and Check now; each installed version tagged current, previous, known-good, in use or bad, with Use and Mark bad; a new version's failed first sessions with Roll back and Keep it ([updates](updates.md#the-clis)) |
| Updates | — | Luna and the core: each version, its state and its button |

- **Waiting marker:** the checkbox turns it on and off; the text under it is
  saved with Save or Enter.
- **Accounts folder:** Change… and Default move where Luna looks for accounts.
  The accounts themselves are moved by hand.
- **Exceptions:** Add an exception… lists the accounts and the project folders
  that have no browser of their own yet; the one picked starts on the default
  browser, and Remove sends it back to the default.
- **Browsers:** a long command line is cut off at the end; Show all shows it in
  full and Copy copies it whole.

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
