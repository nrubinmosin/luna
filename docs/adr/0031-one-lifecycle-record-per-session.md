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
hook missed (`activity/reconcile.rs`, the aging rules of 0.19 kept). Codex: its
rollout's boundaries and subagents, and its job's members born of its turns,
read every 5 s. A helper: its turn's process and stream events, and what its
turns left in its job (`background::life_of`). Codex status hooks, once
trusted, become one more input to the same fold.

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
- What a CLI does not say is not seen. A Claude Code chat's process started
  outside its own tools (`cmd &`) is in no record; a Codex turn on an approval
  dialog reads `running` (active) until its status hooks land; off Windows a
  job lists no members, so a Codex chat's shells and a helper's leftovers are
  not seen there.
- The record's fields are a contract with the GPUI window: changing them
  changes what that window draws. New sources (Codex hooks, app-server events)
  add inputs to the fold, not fields to the record, unless they bring a state
  the record cannot say.
- The waiting marker (a chat waiting because its last line holds a word) is
  gone with the sampler: a CLI's own question or permission is `asking`.
