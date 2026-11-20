# Power and shutdown-when-done

The power field in the status bar opens the two rules the machine follows.
Both go by one test of each session, chat or helper: is it **active**?

## What "active" means

A session is active while any of these holds, as its CLI itself says:

- **A turn runs.** Claude Code says so through its hooks (Luna starts every
  Claude Code session with a `--settings` file adding a few hooks that post to a
  loopback listener; they print nothing, so nothing reaches the model's
  context) and its session file; Codex through its rollout, from the turn's
  start to its end.
- **A turn waits for an answer:** a permission or a question, yours or, for a
  background helper, its agent's to answer.
- **Something runs beside its turns:** a process its turns started that is
  still in the session's job (a build, a dev server, a background shell,
  whatever the CLI's version says of it), a scheduled `/loop` (Claude Code
  lists them as its turn ends), or a subagent at work. It stays active for as
  long as that is there, however long it does nothing: a forgotten `tail -F`
  or an unused server keeps it active until you stop it. What the CLI brought
  up with itself before its first turn (its MCP servers) does not count.
- **Luna cannot tell**: a session just started; a Codex chat before its first
  turn (Codex writes nothing Luna reads until then), or one whose conversation
  file cannot be found or read; a Claude Code chat whose session file cannot
  be told from another chat's in the same folder; a job whose processes cannot
  be listed; a background helper taken up after Luna's core restarted, when
  what its turns left under the old core was not seen to end. Not known counts
  as active: a Codex chat opened and never used keeps the PC awake until it is
  closed or has had a turn.

Nothing is measured: not a process's CPU or disk use, not what a terminal
prints, not what is on its screen. A session whose last turn ended on a usage
limit (or another API error: a lost login, a billing problem) is not active: it
goes nowhere by itself.

A Codex turn held on an approval dialog reads as waiting for you, as Luna's
own hooks in that chat say, or its screen where they do not run. A process that left the session's job altogether is
not seen. A program with a window of its own that a turn started counts too
(a server in a console of its own is a turn's work), and it is not ended with
the session on Windows, where a window tells it is yours; off Windows it goes
with the session.

## Keep the PC awake while sessions work

On by default. Luna holds a Windows power request (`powercfg /requests` lists it
as Luna's) while any session is active, and lets go a minute after the last one
stops being active.

- Windows does not restart its sleep timer for the request: if nobody has touched
  the PC for longer than that timer, it sleeps the moment the hold goes.
- An armed rule (below) holds nothing awake of its own.
- `luna.log` records when the hold was set and let go, which session held it
  last, and every hour which keeps it.

## When every session is done: shut down, restart, hibernate or sleep

Armed for the run, not saved. The chip shows what the rule waits on. Once no
session has been active for the whole chosen window, counted from the arming at
the earliest, it counts down a minute with a notification, and a click on the
chip cancels. Any activity during the window or the count (a turn starting, even
one over within seconds, an agent's call coming and going, a process starting or
ending in a session's job) cancels the count, leaves the rule armed and starts
the window over, and at the end of the minute Luna looks once more before it
acts: better not to shut down than to shut down under work. The window and the
minute are measured on a clock that the PC's time being set does not move; a
sleep, or the time being set, starts the window over.

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
  wake, over a fresh window.
- **Restart** (Windows) goes the same way and has Windows start Luna once at the
  next sign-in, so someone has to sign in; if that cannot be set the log says so
  and the PC restarts anyway. A restart Windows refuses, or records as canceled,
  leaves nothing set to start at the next sign-in; one whose fate is unknown
  keeps it, as the PC may yet restart.
- **Start nothing new** drains: until the rule fires or is disarmed, agents'
  `luna_session_spawn` and `luna_session_send` are refused and `luna_session_answer` takes only a
  structured answer to a pending question or permission. What you do yourself is
  never refused.
- Arming the rule while a session waits for your answer brings a notification
  naming it, and the chip reads `· N waiting for you` while that holds the rule.
- While the rule is armed and something holds it up, `luna.log` says which
  session and why, so a morning with the PC still on can be read back.
- An agent asked to turn the PC off when done arms this same rule with
  `luna_power_set`, and cancels it with the action `cancel`. It is told which
  sessions are active now (`busy`) and which of those wait for a person.

Deleting a chat or helper, or quitting Luna, ends what it left running,
programs with a window of their own aside on Windows; off Windows, where nothing
tells a window from a job, all of it. A core that dies takes a helper's whole
job with it, so nothing runs on unowned.
