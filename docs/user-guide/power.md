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
- **Something runs beside its turns:** a background shell or a scheduled
  `/loop` (Claude Code lists them as its turn ends), a subagent at work, or,
  for a Codex chat and for a background helper, a process its turns left
  behind. It stays active for as long as that is there, however long it does
  nothing: a forgotten `tail -F` or an unused server keeps it active until you
  stop it.
- **Luna cannot tell yet**: a session just started, or one whose CLI has not
  said anything Luna can read. Not known counts as active.

Nothing is measured: not a process's CPU or disk use, not what a terminal
prints, not what is on its screen. A session whose last turn ended on a usage
limit (or another API error: a lost login, a billing problem) is not active: it
goes nowhere by itself.

What a CLI does not say is not seen. A Codex turn held on an approval dialog
reads as running (active, not waiting for you). A process a Claude Code turn
started outside its own tools (a command sent off with `&`) is in no record;
and off Windows, where a job lists nothing, neither are a Codex chat's shells
nor what a background helper left running.

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
one over within seconds, an agent's call that may start work) cancels the count,
leaves the rule armed and starts the window over: better not to shut down than
to shut down under work.

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
windowed programs aside; off Windows, where nothing tells a window from a job,
it runs on. A core that dies takes a helper's whole job with it, so nothing runs
on unowned.
