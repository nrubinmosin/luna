# ADR-0007: A least-privilege contract for agent tools

- **Status:** Accepted
- **Date:** 2026-11-07 (`7c2203e`, revised after review in `cb5d05a`)

## Context

A session with Luna's tools can spawn helpers on any account, type into other
sessions and answer their permission prompts. Before this decision a session in
a read-only or asking mode could spawn a helper in bypass mode, approve its
own helper's prompts, or send a `/` command or `!` shell line into another
chat: a prompt-injected agent could climb out of the mode the user picked for
it. Findings F1 and F2 of the [threat model](../security/threat-model.md).

## Decision

Enabling Luna's tools grants **orchestration authority bounded by the caller's
own privilege**; escalation happens only through an explicit user setting.

- A helper runs at no more privilege than its caller: Claude Code permission
  mode; Codex sandbox, approval and network, compared exactly
  (`agents/privilege.rs` `settle_mode`), unless Settings → Helpers →
  `helpersAboveCaller` is on (off by default).
- No helper is rooted in, inside or above Luna's data dir (`reaches_luna`).
- `luna_answer` with `allow`, raw `keys`, or on a list the CLI holds its screen
  on works only on the caller's own descendants, at the privilege the caller
  would need to run that tool unasked (`answer_reach`).
- Text a CLI reads as a command (`/`, `!`) goes only from a caller at full
  access into its own helper, checked again where queued or kept text is
  finally typed (`command_reach`, `background/queue.rs`).
- Kill and delete reach descendants only; read, wait and plain-text send stay
  open to every session (accepted risk R1: that is how sessions hand work to
  each other).
- A helper's helpers get no tools; at most 8 descendants per chat.

## Consequences

- The contract is documented and tested (the MCP authorization matrix in the
  threat model).
- It is a policy inside Luna, not an OS boundary: it compares recorded modes,
  not effective authority (allow rules, hooks, extra roots), and same-user
  processes can still read tokens. The limits are listed in section 6 of the
  threat model.
- Dialog recognition depends on how each CLI version draws its screen; an
  unrecognized dialog accepts only raw `keys`, which need full access.
