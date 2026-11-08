# Power and shutdown-when-done

The power field in the status bar opens the two rules the machine follows.

## Keep the PC awake while sessions work

On by default. Luna holds a Windows power request (`powercfg /requests` lists it
as Luna's) while any session is busy, and lets go a minute after the last one goes
quiet.

- A session waiting for an answer (yours, or a background helper's question or
  permission its agent is to answer) does not hold the machine.
- What a turn started (a chat's or a helper's) holds it after the turn until it
  has done nothing (under 1% of a core and 1 KiB/s of I/O) for 2 hours on end: a
  client waiting on work elsewhere is quiet too, but a forgotten `tail -F` or an
  unused server is let go then, and stops holding up the rule below. The log says
  so, and it still shows in `luna_list`.
- Windows does not restart its sleep timer for the request: if nobody has touched
  the PC for longer than that timer, it sleeps the moment the hold goes.
- `luna.log` records when the hold was set and let go, which session held it
  last, and every hour which process keeps it.

## When every session is done: shut down, restart, hibernate or sleep

Armed for the run, not saved. The chip shows what the rule waits on; once
everything has been quiet for the chosen window it counts down a minute with a
notification, and a click on the chip cancels. Activity during the count cancels
it and leaves the rule armed.

- **Shutdown** asks Windows first, for a shutdown half a minute away, and closes
  each CLI inside that time. Windows shows its own notice meanwhile. If Windows
  refuses (another user is signed in, say), nothing is closed, and a notification
  and the log say why. The shutdown is a full one whatever Fast Startup is set to,
  and it is forced: a program that will not close does not hold it up, and
  **unsaved work in your other programs is lost**, as Windows' notice says. Save
  what matters before arming the rule. If something cancels the shutdown after
  the chats were closed (`shutdown /a`), Luna sees the cancellation Windows
  records and, about a minute and a half after asking, brings the chats back
  where they were, as after a core restart. If Windows has neither begun the
  shutdown nor recorded a cancellation by then, Luna undoes nothing: the chats
  stay closed and a notification says how to bring them back (restart the core
  from Settings, or quit Luna and start it again), as a slow shutdown may still
  be on its way.
- **Sleep / hibernate** leave sessions running; the rule applies again after
  wake.
- **Restart** (Windows) goes the same way and has Windows start Luna once at the
  next sign-in, so someone has to sign in; if that cannot be set the log says so
  and the PC restarts anyway. A restart Windows refuses, or records as canceled,
  leaves nothing set to start at the next sign-in; one whose fate is unknown
  keeps it, as the PC may yet restart.
- **Start nothing new** drains: until the rule fires or is disarmed, agents'
  `luna_spawn` and `luna_send` are refused and `luna_answer` takes only a
  structured answer to a pending question or permission. What you do yourself is
  never refused.
- While the rule is armed and something holds it up, `luna.log` says which
  session and why, so a morning with the PC still on can be read back.
- An agent asked to turn the PC off when done arms this same rule with
  `luna_power`, and cancels it with the action `cancel`.

## What "busy" and "done" mean

Busy is measured from three signals per session:

1. **The turn:** Claude Code's own hooks and registry, Codex's rollout. To hear
   turn boundaries Luna starts every Claude Code session with a `--settings` file
   adding a few hooks that post to a loopback listener; they print nothing, so
   nothing reaches the model's context.
2. **The processes under the CLI**, put in a Job Object at spawn: a build, a test
   run or a `sleep` left in the background counts while it moves or was started
   by a turn. What the CLI brings up with itself (MCP servers, its shell) counts
   only while it moves; a program with a window of its own (a browser a login
   opened) not at all.
3. **Recent output.**

A background helper's leftovers between turns count the same way. Deleting a chat
or helper, or quitting Luna, ends what it left running, windowed programs aside;
off Windows, where nothing tells a window from a job, it runs on. A core that dies
takes a helper's whole job with it, so nothing runs on unowned.

**Done** means no session is busy or waiting for a permission. A session whose
turn ended on a usage limit (or another API error: a lost login, a billing
problem) and sits on that dialog counts as done, and the countdown notification
says how many there were. Arming the rule while a session waits for your answer
brings a notification naming it, and the chip reads `· N waiting for you` while
that holds the rule.
