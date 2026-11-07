# ADR-0012: Agents drive sessions over MCP, and answers come back as notices

- **Status:** Accepted
- **Date:** 2026-09-24 (`3efaab9`, the MCP tools); notices typed into the caller
  since 2026-10-08 (`298ade6`)

## Context

A model should be able to start another session (another model, account or
CLI), talk to it and collect its answer. Two questions: how the model learns of
Luna, and how an answer reaches a caller that ended its turn instead of
waiting. A system-prompt instruction costs context in every session, touches
the user's prompt, and differs per CLI. A helper, unlike a CLI subagent, does
not report back on its own: its answer sits in its transcript.

## Decision

- Luna is an **MCP server** on the hub, given only to sessions with the "Luna
  tools" box ticked. Its instruction travels in the `initialize` answer's
  `instructions`; the tools and that line cost about 2k tokens once. The full
  manual is `luna_guide`, read on demand. Sessions without the box know nothing
  of Luna.
- Every question (`spawn`, `send`, `answer`) records a **debt** (`agents/owed.rs`).
  Only an outcome that carries the answer settles it: a `wait` that returns the
  reply, or a full, uncut `read` of a finished helper.
- The **notifier** (`agents/notifier/`) types a one-line
  `[Luna notice, not typed by the user]` into a caller that is between turns
  and not being typed into, naming the event and a file holding the full
  answer, report first.
- A circuit breaker stops turn-end notices for a (caller, session) pair past 12
  in 15 minutes, until a person types into either chat.

## Consequences

- An orchestrator can fan out and end its turn; it is woken when there is
  something to read.
- The protocol has many edge cases (drafts, a `wait` canceled mid-response,
  queued text, core restarts), documented in
  [agents](../architecture/agents.md); its state is the most delicate part of
  the core ([ADR-0010](0010-registry-encapsulation.md)).
- Notices are ordinary turns of the caller: they cost tokens, and loops between
  two models are possible; the breaker and "riders" (minor news waits for the
  next real notice) exist for that.
