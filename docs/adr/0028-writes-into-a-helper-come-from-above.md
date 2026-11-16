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

**What a move does.** One window command, `background_move` (agents/reparent.rs
`move_helper`), never an MCP tool. It moves a helper that is a direct child of
one of the person's chats, or one with no chat above it, to another of the
person's chats that runs with Luna's tools; never into the helper's own line,
never under a lead or a helper. The new chat's privilege has to cover every
session that moves, as a spawn by that chat would have had to (`privilege.rs`
`holds_over`, with `helpersAboveCaller` the one way past it), and its line has
to have room for them and for the spawns under way below them, counted in the
same hold of the registry a spawn counts its places in. A helper being opened
as a chat, being deleted, or between a lead's handoff and its fresh start is
not moved. Every refusal names why, and nothing has changed by then; the
window asks `background_move_targets` for the same verdict per chat, to gray
out the ones it may not choose.

In one move:

- the helper's parent becomes the new chat and its orphan mark goes; the
  helpers under it keep theirs, and so follow;
- what the old chat had of the moved sessions goes: its debts, requests,
  receipts (a canceled call cannot put a debt back), what it was last shown,
  its answer marks and notice counts. It hears one line saying where the
  helper went, that nothing more is owed by it, that it may still read and wait
  with `scope: "all"`, and how many of its messages are still queued for it.
  Its later sends, answers, stops and deletes on any of them are refused
  naming the new chat (`moved_error`, the pattern of `taken_over_error`), as
  are its reads and waits without `scope: "all"`: a wait in flight sees that at
  its next look and ends refused, a wait with `scope: "all"` goes on;
- the new chat is owed the helper's answer, at the turn count it stands at,
  when the helper is in a turn, has text queued or holds a question or
  permission: the answer it is at work on now goes to the chat it answers to;
- an `autoDelete` that its old chat's deletion switched on (`pass_up`, kept
  apart as `Auto::by_pass_up` and in its ties) is switched off, as the new chat
  is there to delete it; one its spawner asked for stays;
- the chats that had been refused a write into it are no longer offered as
  places to move it, and the move is kept in its ties ("moved from A to B,
  when"), for the window's row and for the refusals, across a core's restart.

**Order.** The move holds the notifier's hand-off on every session that moves,
so a notice about one of them going in is in before the debts change; then
whose the helpers are, alone (`registry.rs` `ownership_changing`), which every
send and answer into a helper holds shared from its first check
(`authorize_send`) to the debt it leaves (`owe_from_at`, `owe_from`); then the
helpers between turns (`background::claim_move`), so no turn starts until the
move is through and the next one starts under the new parent; and the checks
and the change in one hold of the registry. A send or an answer the old chat
had checked is in, debt and all, before the move closes that debt; one that
comes after is checked against the new parent. A background send does not go
through the notifier's `going_into` hold, so the hand-off alone would not have
been enough.

**Each CLI, moved mid-turn or between turns.** The CLI is not restarted. Its
next turn is started with `LUNA_PARENT_ID` naming the new chat, as after a
`pass_up`, and with a note in its prompt that the person moved it and to which
chat (`move_note`, given once; background/command.rs `stdin_with`): Claude
Code has its role note in its first prompt only, so this note is how a later
turn learns it, and Codex has it beside its role note, which it gets every
turn. Text queued before the move still goes in: into a Claude Code helper's
running turn on its stdin while that turn takes text (it joins at the next tool
boundary), else at its next turn; a Codex helper reads its stdin once
(docs/cli-compatibility.md), so all of it waits for the next turn. Text the old
chat had queued as its own helper's has no `[From session …]` opening; text it
had queued that the CLI would read as a command is dropped at delivery, as its
sender may no longer send it there, and the old chat is told
(`background/queue.rs` `settle_batch`). A question or permission the helper
holds is the new chat's to answer from the moment of the move.

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
- A move is the person's alone, and it cannot widen what a session may do:
  the new chat already reaches every one of the person's chats' helpers for
  reading, and must cover the moved sessions' privilege and have their places.
  Its negative tests are in `agents/reparent_tests.rs`, the ordering against a
  send and an answer included.
- Every write into a helper takes a shared hold that a move takes alone: a
  move waits for sends and answers in flight (a background send starts the
  helper's turn under it, an answer waits out its settle), and sends wait for
  the moment a move takes.
- Nothing here depends on the helper's CLI: the parent link, the check and
  the refusal live in Luna's registry, and a Codex helper is refused as a
  Claude Code one is.
