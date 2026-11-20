# ADR-0032: Run each Codex helper turn as one `codex app-server`, steered and stopped through it

- **Status:** Accepted
- **Date:** 2026-11-20

## Context

Until 0.20 a Codex helper's or lead's turn was one `codex exec --json` run:
the prompt on stdin, read once, the events on stdout. It had no way in once
started: text sent mid-turn waited for the next turn, a stop killed the
process tree, and `exec` asked nobody, so Luna forced `approval_policy =
"never"` and a reviewer of its own. ADR-0030 (part C) moves Codex helpers to
`codex app-server`, experimental in 0.169.0: JSON-RPC 2.0 on stdio, with
`thread/start` / `thread/resume`, `turn/start`, `turn/steer`,
`turn/interrupt`, and approvals and questions sent to the client as requests.
A live probe on 0.169.0 (`test-build/appserver-probe/notes.md`) saw all of
them work, in a folder without git too, and a server exit within 0.1 s of
its stdin closing.

Three process models were on the table: one server a turn, one a helper for
its whole life, and one an account shared by its helpers.

## Decision

**One `codex app-server` a turn, per helper** (`background/appserver.rs`,
`background/run.rs`). A turn starts the server with the `-c` overrides an
`exec` had (MCP, connectors off, trust, sandbox, the approval its parent
named), sends `initialize`, then `thread/start` (a first turn: `cwd`,
`model`, the role layers as `developerInstructions`) or `thread/resume` (the
same, by the thread's id), then `turn/start` with the text and the effort.
At `turn/completed` Luna closes stdin and the server goes; the turn ends with
the process, as every helper turn does. Not one a helper: an idle helper
would hold a server (memory, the version directory a CLI update or rollback
needs free) and a process that reads as work in the lifecycle record, and a
helper's turns would share a crash. Not one an account: Luna's MCP server is
given per helper, with its own token, by `-c` at the server's start. Every
turn after the first resumes, so resuming after a core restart is no
special path.

**Steer, else queue** (`queue.rs` `write_line`). A `luna_session_send` while
the turn runs and has an id goes in as `turn/steer` and is delivered when the
server answers; one the turn does not take (it ended first: "no active turn
to steer") starts the next turn, as text a Claude Code helper never took in
does. A notice, a send before `turn/start` has answered, one while the turn
is being stopped, and one behind queued text queue, as before.

**Interrupt, then kill** (`control.rs`). `luna_session_stop` and `send` with
`interrupt` send `turn/interrupt`; the turn's `turn/completed` (status
`interrupted`) ends it. Items the server leaves open then are closed in the
helper's log. No end within 5 s (`INTERRUPT_GRACE`): the process tree is
killed, as before. A server still up 10 s after its stdin closed is ended,
and only it. Luna's own stops (its going, an update) still kill.

**Asks go to the caller** (`appserver.rs` `server_request`). The server's
`item/commandExecution/requestApproval`, `item/fileChange/requestApproval`,
`item/permissions/requestApproval` (and the v1 `execCommandApproval`,
`applyPatchApproval`) are held as permissions, `item/tool/requestUserInput`
as a question, exactly as a Claude Code helper's `luna_permission` calls:
`ask` in its record and row, notices, `luna_session_answer`, ASK_TIMEOUT.
The answer is the reply to the server's request (`accept` / `decline`, the
answers by question id); no answer is a refusal. An MCP server's
elicitation is declined and any other request refused at once. Helpers do
not run in plan mode: it is the only mode where 0.169.0's model has the
question tool, and it plans instead of doing what it was asked.

**Approval as the parent names it.** `never`, `on-request` (`on-failure` as
it) or `auto-review`; none named, Codex's own config decides (ADR-0030's
amendment), closed to `on-request` where Luna cannot read it. Luna adds no
reviewer of its own.

## Consequences

- `codex exec` is no longer run: its argv, its event parser and their tests
  are gone. The Codex floor rises to 0.169.0, the protocol Luna speaks.
- The protocol is experimental: every new Codex version runs the live suite
  against it (the cli-upgrade skill), and a changed shape shows as a helper
  whose turns fail to open.
- A parent now hears its Codex helper's approvals as asks and has to answer
  them, where a helper spawned without `approval: never` used to run unasked.
- Each turn pays the server's start and a `thread/resume` (about 0.1 s), as
  each `exec` paid its start.
- A steer is the model's to read at its next step: a parent cannot know it
  changed what the turn was doing, only that the model was given it (its
  `userMessage` item); one an interrupt drops goes back to its sender.
- A thread is the helper's only once its first turn has started; the
  approval and sandbox Luna passes go typed and are checked in the answer
  (fail closed); messages carry `clientUserMessageId`, which the rollout
  keeps, so a core's death settles each text sent with no word back by the
  thread: there, delivered; not, handed back as one that may not have
  arrived. Luna sends nothing again by itself, a first turn's opening
  included (the server may have left an orphan thread); a server that breaks the protocol, or one that does not
  start its turn within 3 minutes, fails the turn and counts against its
  version in the canary.
- Luna ends no started turn by the clock: a Codex turn can be quiet for a
  long time inside one long command. Its parent hears of the silence as of
  any helper's ("written nothing for N min", agents/notifier
  `QUIET_TURN_MS`) and decides to interrupt or delete it.
