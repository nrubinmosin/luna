# ADR-0010: Encapsulate the agents Registry behind transition methods

- **Status:** Proposed
- **Date:** 2026-11-08

## Context

Everything the agent layer knows sits in one `Registry` behind one
`Mutex` (`src-tauri/src/agents/registry.rs`, reached through `reg()`). It has
about 25 fields, all `pub(super)`, and the modules of `agents/` lock it and
mutate fields directly. The four maps that carry the answer-delivery protocol
are the most delicate:

| Field | Meaning | Direct uses today |
| --- | --- | --- |
| `owed` | answers owed, by (caller, session) | about 160, in 14 files |
| `waits` | `luna_wait` calls in flight | 7, in 3 files |
| `reserved` | sessions a `wait_many` in flight answers for | 4, in 2 files |
| `receipts` | the latest hand-over, the one a cancel may undo | 4, in 2 files |

Their invariants (a debt is settled only by an outcome that carries the answer;
a hand-over and its undo touch only the revision they saw; reserved sessions are
held from the notifier; a cancel never loses an answer) are kept by convention
across files. A review of the phase-2 split suggested starting the
encapsulation with these four.

## Decision

Move `owed`, `waits`, `reserved` and `receipts` into a private sub-struct
(working name `Debts`) owned by `Registry`, reachable only through transition
methods that run under the **same** registry lock the caller already holds:
for example `owe`, `take_debt`, `settle_told`, `hand_over` / `undo`, `reserve` /
`release`, `wait_started` / `wait_ended`. Each method states and checks its
invariant (`debug_assert!` where cheap), and the unit tests move to that type.

Rules:

- No second lock: the methods take `&mut Registry` (or `&mut Debts`) inside an
  existing guard, so lock order and atomicity with the rest of the registry
  stay as they are.
- No behavior change in the same commits; golden and unit tests must pass
  unchanged.
- Other fields follow one group at a time (spawns, resumes and openings next),
  once the pattern has held.

## Consequences

- The delivery protocol becomes readable in one file, and new callers cannot
  bypass it.
- The change is wide (about 160 call sites for `owed`), so it lands in
  mechanical steps, one method at a time.
- It is the precondition for extracting `agents/` as a crate
  ([ADR-0009](0009-modules-now-crates-later.md)).
