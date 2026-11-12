# ADR-0025: A helper reaches only its own subtree

- **Status:** Accepted; narrows the accepted risk R1 of the
  [threat model](../security/threat-model.md#8-accepted-risks) and the reach
  [ADR-0012](0012-agents-over-mcp-with-notices.md) gave every tooled session
- **Date:** 2026-11-13

## Context

Any session with Luna's tools could read and wait on every session, send plain
text to any other and answer its model's questions, the person's chats
included, once its call said `scope: "all"` (R1). That reach was meant for the
person's own chats, which hand work to one another. Helpers spawned with
`tools: true` (leads) have it too, and in use they have become common:
an orchestrating chat runs several leads, each with helpers of its own.

A lead is the session most exposed to prompt injection (it reads code,
reviews and tool output all day) and the furthest from a person. With
`scope: "all"` an injected lead could type into the person's chats, read
another line's transcripts, raise desktop notifications in the person's name
or arm a PC shutdown. Nothing a lead legitimately does needs any of it: its
work comes from its parent and goes back in its reply, which the notifier
carries up.

The cap on places was also the line's (8 under the top chat, every generation
counted) but neither the spawn's result nor the listing said how many were
left, and a lead refused for want of one could free none of the others, while
the chat that could heard nothing.

## Decision

Only a person's chat reaches past its own subtree
(`agents/registry.rs` `persons_chat_in`: no parent, not an orphan, not run in
the background; or the dev caller, which stands in for the person):

- `scope: "all"` from any other caller is refused in every tool
  (`scope_all_in`, `SCOPE_ALL_REFUSED`), saying what is allowed.
- Without it, `in_scope`, `authorize_view` and `authorize_send` (`beneath`)
  refuse a helper any session not under it, its parent and the person's
  chats among them; `luna_session_list` and `luna_session_get` refuse it ids
  outside its subtree (`agents/list.rs` `ids_in_reach`), and a spawn's
  `sameFolder` names none past it (`neighbors_in`). Stop, delete and
  `allow` were already descendants only.
- `luna_notification_send` and `luna_power_set` go only from a person's chat
  (`mcp/dispatch.rs` `persons_chat_only`).

Places are the person's to set (`Settings::max_helpers`, 1 to 32, 8 by
default) and said: `places` (`inUse`, `of`, `underYou`) in the spawn's result
and in `luna_session_list`. A refusal names only what the caller can delete,
and the top chat hears once per burst of refusals below it
(`agents/spawn/mod.rs` `refused_a_place`).

## Consequences

- A prompt injected into a helper stays in that helper's tree: it can reach no
  person's chat, no other line and not the person's desktop or power. R1 now
  holds for the person's chats only.
- A helper cannot message its parent or a sibling directly; its reply is the
  channel up, and a sibling's work reaches it through the parent. A lead that
  used `luna_session_send` upward, or read a sibling's transcript, has to be
  briefed otherwise.
- A helper can no longer notify the person; it has to say so in its reply and
  leave it to the chat above.
- Every verb's refusal is tested from a lead (`mcp/mod.rs`
  `a_helper_reaches_only_its_own_subtree_in_every_tool`,
  `agents/registry.rs` `reaching_past_ones_own_takes_scope_all`,
  `lineage_and_reach`); a new tool that targets a session has to go through
  `in_scope`, `authorize*` or `ids_in_reach`, or the matrix in the threat
  model (§5) is broken.
- More places are more CLI processes and tokens under one chat; that is the
  person's choice, and the range stops a mistyped value from taking none or
  hundreds.
