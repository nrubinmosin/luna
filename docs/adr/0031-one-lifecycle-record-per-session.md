# ADR-0031: Keep one lifecycle record per session, from the CLIs' own signals

- **Status:** Accepted
- **Date:** 2026-11-20

## Context

Until 0.19 a 5-second sampler judged each session busy, waiting, idle or
stalled from three signals: the turn (hooks, registry, rollout), the
processes under the CLI (CPU and I/O rates, children born after the first
turn), and the screen (output in the last 30 s, a Codex screen still for 3 s
mid-turn read as a prompt). What a turn left running held the PC until it had
done nothing for two hours. Every consumer drew its own view from parts of
that: the power rule from the sample's `busy`, the agents' `wait` from the
turn count, the rows from the registry's word overlaid by a waiting marker,
`reap` and the core's idle exit from a leftover's rate. The measurements
disagreed with each other and with the CLIs (a repaint after a resize read as
work; a polling `tail -F` held a PC all night; a quiet Codex turn read as a
prompt), and the GPUI window (ADR-0030) needs one thing to read.

Claude Code says what it does: its hooks (a turn begun, a permission or
question up, `Stop` with the `background_tasks` and `session_crons` still
there, `StopFailure`), and its registry file `sessions/<pid>.json` (status,
and from 2.1.327 `shell` for background Bash). Codex says less today: its
rollout's turn boundaries and its subagents' rollouts; its status hooks come
later (ADR-0030, B), and the shells a turn leaves are in the session's own
job.

## Decision

Each session, a chat's or a helper's, of either CLI, has one record
(`activity::Life`, `activity/record.rs`): a phase (`running`, `asking`,
`ended`, `unknown`), the last turn's outcome (`ok`, `failed`, `usageLimit`)
and error, when the turn on now began, what runs beside the turns
(`background`: shells, crons, subagents, by name where Luna can tell), and the
completed-turn count, which only grows and is kept apart from liveness.
`Life::active` is the one liveness test: anything but an ended phase, or
anything beside it; `unknown` is active.

The record is made of what the CLIs say and nothing measured. Claude Code: its
hooks at once (`activity/hooks.rs`), its registry read every 5 s to settle a
hook missed (`activity/reconcile.rs`), each word aging only from the later of
the hook's and the registry's own time. Codex: its rollout's boundaries and
subagents, and an approval dialog on its screen while its rollout says a turn
runs, read every 5 s. Both: the session's job, listed every 5 s, against a
snapshot of what was in it before the first turn (the CLI's own furniture),
so a background shell is seen whatever the CLI version says of it. A program
with a window of its own counts as one: a console a turn opened for its server
is its work, and only a person can tell it from a program of theirs, so
liveness errs active (ending errs the other way and spares it: `procs` looks
for a window only to decide what Luna ends). A helper:
its turn's process and stream events, and what its turns left in its job
(`background::life_of`). Off Windows the job is the keeper's
(`keeper::listed`: the turns' processes, their groups and sessions, what
carries the job's mark). Codex status hooks, once trusted, become one more
input to the same fold.

**Missing evidence is unknown, never idle.** A session no source has spoken
for yet (no hook, no registry entry, no rollout: a Codex chat before its first
turn included); a Codex rollout unreadable or with no turn event; a Claude Code
registry entry matched only by folder that is not the one live entry there; a
job that will not list; a helper taken up after a core restart whose old job
was not seen out (by that core's tie, or by `stop_all` as it left): each reads
`unknown` (or, beside a turn, `background.unlisted`), which is active. The
cost is taken: a Codex chat opened and never used holds the PC awake.

**One monotonic activity stamp.** Power does not judge from a board alone.
Every signal moves one stamp on the monotonic clock (`activity::last_activity`,
an `Instant`, never wall time): any hook; a read whose inputs changed (a
registry word, a rollout growing, a job's members, a turn count) or that found
a session active; a helper's turn claimed, about to start or ended, or its
job changing; an agent's call that may start work coming or going; Luna's
text into a pane; a person's key into one. The quiet window and the
countdown both count from it, so activity between two reads cancels. The
countdown's end is decided behind a barrier (`power::barrier`): admission
closes first (agents' calls refused, helper turns and Luna's writes into panes
too, each of which moves the stamp before it asks), then a fresh read
(`power::confirm`), then the stamp; it fires only with no session active and
the stamp no later than the countdown's start, else admission opens again.
Evidence is never ordered by the wall clock against Luna's own: a Codex line
is placed by where the file ended at the spawn where Luna saw it, and a
Claude Code registry idle counts against a hook only when set after it. A
sleep or a clock step (the monotonic gap past six reads, or the wall clock
drifting 30 s from the monotonic one between reads) starts the window over,
and so does one across the countdown itself, which the barrier checks on both
clocks, as a suspend during its read falls between no two reads. A panic in the
read or the firing opens admission again.
This shape was chosen over point fixes after review found wrongful-shutdown
paths in the per-read board.

Every consumer derives its view from the record: the power rule (any active
session holds the PC awake and a power-off back), `luna_session_wait` and the
notices (`TurnState::of`), the rows' status (`Life::status`), the delete
warnings, autoDelete, the core's idle exit and `Welcome.left_only`. The
record is serialized as is on the power board (`power://state`'s
`summary.sessions[].life`), the shape the GPUI window reads.

## Consequences

- No CPU, I/O, output or screen is read for liveness; `procs` reads only who
  is in a job and when each started. A leftover holds the PC for as long as it
  exists: a forgotten `tail -F` keeps it on until it is stopped, which is the
  price of never shutting down under a job that waits on work elsewhere.
- Residuals, each erring active or limited to the session that causes it:
  a process a turn started that left its job (a Windows breakaway, a Unix
  process that cleared its environment and lost its parents) is not seen; a
  Luna text typed into a pane, a notice or a send, is let in or refused as a
  whole before its first key and counted until its last, and a helper turn the
  gate refuses waits with its batch and starts when admission opens; a
  hook or a key that comes between the barrier's read of the stamp and the
  OS's answer is not waited for (nothing a person does is refused, and a
  turn it starts may be cut by the shutdown, with the 30 s grace and
  `shutdown /a` to take it back); a Claude Code older than `statusUpdatedAt`
  that misses a `Stop` holds its record active until its next hook; a Codex
  chat moved to an older thread in its TUI (`/resume`) whose last turn there
  was cut off reads `unknown` until a turn of its own ends, as nothing dates
  that turn against the spawn but the clock; a known subagent's rollout that
  will not read holds its session for as long as that lasts; a helper turn in
  no job leaves its walk by parent links, which misses what reparented away
  from its tree; a helper taken up after a core restart whose old job was not
  seen out has no job and no pid to look at, so it reads active until its next
  turn or until `luna_session_stop` or a kill, which walks what it can and,
  finding nothing, settles it as gone (anything it left running goes unseen
  then); a Codex subagent whose rollout is gone (the file is not there: not a
  read that failed) once its session's own turn has ended is ended with that
  turn, as Codex writes no end for a thread it archives, and stays not seen
  while the turn runs; a Claude Code resume found by session id counts only
  with its process running and a terminal session's entrypoint; a `Stop` whose turn's start was missed and that comes after
  a read counted the turn before is taken for that turn's, late, and counts
  nothing (a `wait` then runs to its timeout); a
  Codex MCP server started after the last read before a first turn that
  itself began before any read is counted as a shell off Windows, where start
  times cannot date it against the turn (active, never quiet, while it runs);
  with two Claude Code chats in one folder whose own registry entries are
  not found by pid, both read `unknown` for as long as that lasts; a session
  that posts its own `Stop` hook with its secret can end its own turn in its
  record (threat model); a program with a window of its own that a turn started
  counts while it runs and is spared when the session is ended on Windows, as
  the person's to close (off Windows nothing tells it from a job, and it goes
  with the session).
- The record's fields are a contract with the GPUI window: changing them
  changes what that window draws. New sources (Codex hooks, app-server events)
  add inputs to the fold, not fields to the record, unless they bring a state
  the record cannot say.
- The waiting marker (a chat waiting because its last line holds a word) is
  gone with the sampler: a CLI's own question or permission is `asking`.
