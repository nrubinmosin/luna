# Sessions and cleanup

## Closing, quitting, restarting

- Closing the window hides Luna to the tray; sessions keep running.
- **Quit** from the tray ends all of Luna: every session is asked to quit as
  deleting a chat would, background helper turns are stopped, then the core and
  the window go. A core still there 30 seconds later is killed with everything
  it started.
- **Restart window** in the tray starts only the window over (terminals drawing
  garbage, say): the core and its sessions keep running, turns in progress
  included, and the new window picks them up.

## The core

Sessions run in Luna's **core**, a second process from the same exe
(`luna-core.exe`, started from a copy under the data folder on Windows so an
update can replace the exe), not in the window.

- A window that crashes, hangs or is killed takes nothing with it: the CLIs
  carry on and the next window reattaches. A window that falls behind on output
  is let go by the core and comes straight back, terminals redrawn from the
  scrollback.
- The hooks' listener, the agents' MCP server, activity and power run there too,
  so an agent's `luna_wait` keeps working with no window open.
- Settings → Versions shows the core and can restart it. It leaves by itself
  after ten minutes with no window and no session (a background helper in a
  turn counts, and so does one whose leftovers still keep the PC awake).
- If the core stops, its sessions end: a banner says so, and **Restart the
  core** brings the chats back from their transcripts. It is never restarted
  behind your back.

## Helpers across a core restart

Background helpers outlive the core: each is kept in `helpers.json` in the data
folder (its settings, its conversation's id, whose it is, and any message not yet
delivered to it; the conversation itself stays in the CLI's transcript), and the
next core takes them up after an update, a restart or a reboot.

- A turn the core was in the middle of is stopped with it. The agent owed the
  answer is told so once its own chat is back, along with any messages queued
  behind that turn, kept word for word; `luna_send` has the helper carry on.
- With `resumeOnRestart: true` a helper goes on by itself, those messages
  included, where Luna knows the old turn ended (a normal stop, or any death of
  the core). One stopped before its first turn got anywhere is reported as never
  started, with its prompt, and with `resumeOnRestart` starts afresh on it.
- A visible helper with `resumeOnRestart` goes on too: Luna starts its CLI, in a
  pane or not, and types the same word in once it is back at its prompt, unless
  you typed into it or an agent wrote to it first. A visible helper without it
  that has no conversation to come back to is reported as never started, its
  prompt and the agent's held messages in a file.
- Luna names a Claude Code helper's worktree itself, so a fresh start goes back
  into that one only, and only if nothing has touched it since Claude Code made
  it. Anything else, and whatever a Codex helper began (Codex takes no name),
  stays where it is and the agent hears of it as `worktreeLeft`.
- What agents hold of your visible chats (who spawned them, a worktree they were
  handed, answers owed, messages held for your draft) is kept in
  `chat-ties.json` and picked up when each chat starts again. So is a
  `luna_spawn` the core went in the middle of: the agent is told no helper came
  of it, with its prompt in a file and any worktree it had begun named and left,
  or with `resumeOnRestart` Luna starts it afresh and says so.

## Orphans and leftover worktrees

- Sessions with no chat row left to reach them (lost to a half-written state
  restore, say) are listed at the top of the sidebar, with what they work on,
  and can be adopted back or killed.
- **Worktrees** on the title bar lists worktrees Claude Code and Codex left
  behind with no chat in them, by folder and CLI, with a count on the button.
  Each is looked inside first: clean ones start ticked; ones with uncommitted
  files or commits that deleting would lose (on no branch that stays, nor a
  remote) say so. Delete one by one or all ticked; nothing a live session or
  helper sits in is ever deleted.
- Deleting a chat takes its worktree, and the branch Claude Code made for it,
  by default, unless another chat or helper still works there. The checkbox can
  be unticked per chat; where it starts is set in Settings.

## The log

Both halves log to `luna.log`, next to the exe, or under `%LOCALAPPDATA%\luna`
when that folder is not writable: warnings and errors, and the events a problem
is read back from (each session started, exited and deleted, each helper turn,
each MCP call with who called which tool on which session, how it ended and how
long it took, each notice, the awake hold set and let go). Sessions are named by
id and chat name, so a deleted one can still be told apart. What is said in a
chat never goes in. Every line names the version; each launch opens with a
`START` line naming version and build.

The log also says how each run ended: Luna's own reasons (tray Quit, an update,
Windows signing out or shutting down); a small watcher logs the exit code when
something else ends it, telling a kill from outside (Task Manager, `taskkill`,
`Stop-Process`) from a crash, and whether Task Manager was open. If even the
watcher is gone, the next launch reports when the previous run was last alive,
whether its window had frozen first, and whether the PC restarted in between.
