# ADR-0028: Only the sessions above a helper write into it; the person moves it

- **Status:** Accepted; narrows the accepted risk R1 of the
  [threat model](../security/threat-model.md#8-accepted-risks), after
  [ADR-0025](0025-a-helper-reaches-only-its-own-subtree.md) narrowed it to the
  person's chats
- **Date:** 2026-11-17

## Context

R1 let any of the person's chats with Luna's tools read, wait on, send plain
text to and answer the model's questions of any session once its call said
`scope: "all"`, another chat's helpers included. It made no difference
between two cases:

- **a chat writing to another chat**: a peer talking to a peer, which the
  person sees and answers;
- **a chat writing into another chat's helper**: text lands in a session
  working under a brief, constraints and an account another parent chose.
  That parent is not told of it and does not see it; the helper's report and
  its turns' notices still go to it. A prompt-injected or confused chat can
  steer the helper mid-round.

In use it happens without any injection (issue #72): chat A runs a long
workstream with a helper, its context fills, and the person starts chat B
from a written handoff. B drives the helper with `scope: "all"`, while to Luna
it is still A's: it sits under A in the window, A gets its notices, and
deleting A either kills it mid-round or leaves it parentless with
`autoDelete`, gone 30 minutes after its answer. Nothing could hand a helper
from one chat to another: a parent was set at spawn and changed only when it
was deleted (`pass_up`) or the person opened the helper as a chat of their own
(`handover.rs`).

What B was told of its own writes: every send and answer makes a debt for its
caller, not only for the parent (`owed.rs` `owe`, through `owe_from_at` in
`delivery.rs` `send` and `owe_from` in `answer.rs` `answer`), and the
notifier tells it of the turn that answers its text, naming the helper as a
session "which you wrote to" (`notifier/mod.rs` `run`). What it did not get:
the helper's later turns, its report and its questions, which go to the
parent.

On the table:

1. Keep R1 and show it: a "driven by" mark on the helper, a notice to the old
   parent when another chat writes into its helper.
2. Limit writes into a helper to its ancestors and add a hand-over, either an
   agent-callable `luna_session_adopt` (or a `parent` argument on an existing
   call) or a move the person makes in the window.
3. Have a recent `scope: "all"` writer become the parent on its own when the
   parent is deleted.
4. Turn `autoDelete` off for a helper orphaned by a delete: it would not be
   lost, but its notices would still go nowhere and it would have no owner in
   the window.

## Decision

**Writes into a helper go only from above it.** `luna_session_send` and
`luna_session_answer` into a session that is not a person's chat take the
caller to be a strict ancestor of it (`is_descendant`): its parent, a lead
between, the chat at the top. A sibling, another chat and that chat's helpers
are refused, `scope: "all"` or not; a helper with no chat above it (an
orphan, one run in the background with no parent) takes no agent's writes at
all. One check carries it, `agents/registry.rs` `write_reach`, returning why
it refuses (`WriteRefused`); the tool's gate (`in_scope` for the `send to`
and `answer` verbs, which `mcp/dispatch.rs` passes) and the write itself
(`authorize_send`, which `delivery.rs` `send` and `answer.rs` `answer` call)
both ask it, and so does any later step that changes whose a helper is under
a write. Unchanged: a person's chat writing to another of the person's chats
(plain text, and answering its model's questions), so R1 stays for peers;
reading and waiting with `scope: "all"`, which is how a chat keeps an eye on
work it does not run; and the dev caller, which stands in for the person and
which the live suite drives helpers through.

**The refusal says what to do.** A person's chat refused hears whose the
helper is (its parent and the chat at the top, by window name with id), that
only the sessions above it send to it or answer it, that it may still read
and wait on it with `scope: "all"`, and that the person can move the helper
to this chat in Luna's window ("Move to chat…" in the helper's menu). A helper
refused hears `not_under_you` as before and nothing of a move, which it cannot
ask the person for. A former parent of a helper the person took over hears the
takeover's words (`taken_over_error`), as before.

**No agent-callable adopt tool. The person moves a helper.** An agent tool
that re-parents would be the same steering in two calls: an injected chat
adopts, then writes. Instead the person moves a helper, with everything under
it, to another of their chats from the window ("Move to chat…" on the helper,
or a choice in a chat's delete dialog). Only the helper's parent changes, so
its subtree follows, and its id, worktree, settings and constraints stay. A
write refused for want of this records the chat that wanted the helper, and
the helper's row offers the person "Move to <that chat>?"; only the person's
click moves it.

Not taken: keeping R1 with a "driven by" mark and a notice to the old parent
(option 1) shows the steering but leaves it possible, and the parent hears of
it only after the text is in; an agent-callable adopt (option 2's first form),
for the reason above; a recent writer becoming the parent on its own when the
parent is deleted (option 3), as the person, not the order of calls, decides
who runs a helper; turning `autoDelete` off for orphans alone (option 4), which
saves the helper but leaves it with no one to tell.

## Consequences

- An injected or confused chat can no longer steer another chat's helper:
  its text, a model question's answer and a permission's answer all stop at
  the refusal, and what the helper holds stays held for its own chat
  (`mcp/mod.rs` `another_chats_helper_is_written_into_only_from_above_it`,
  `agents/registry.rs` `writes_into_a_helper_go_only_from_above_it`,
  `reaching_past_ones_own_takes_scope_all`).
- The written-handoff workflow needs the move: chat B reads and waits on A's
  helper as before, but drives it only once the person has moved it under B.
  The refusal says so, so a chat that tries is pointed at the person.
- A chat that wants to tell another chat's helper something has to go through
  that chat (a send to the chat, which is still open), or ask the person.
- Every new path that writes into a session, or changes whose a helper is,
  has to ask `write_reach` at the moment it commits, or the matrix in the
  threat model (§5) is broken.
- `[From session <id> via Luna]` now opens text only from one of the person's
  chats to another (and from the dev caller): every allowed write into a
  helper comes from above it, which writes as its own (`as_sent`).
- Nothing here depends on the helper's CLI: the parent link, the check and
  the refusal live in Luna's registry, and a Codex helper is refused as a
  Claude Code one is.
