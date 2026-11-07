# ADR-0009: Modules now, crates later

- **Status:** Accepted
- **Date:** recorded 2026-11-08; the module splits it describes landed on
  2026-11-07 (`refactor/phase-2`, e.g. `agents.rs becomes agents/mod.rs`)

## Context

The Rust side is one crate, `luna` (`src-tauri/Cargo.toml`), with a few files
that had grown past several thousand lines (`agents.rs`, `background.rs`,
`library.rs`, `pty.rs`). Two ways out: split the crate into a workspace of
crates (`luna-agents`, `luna-pty`, ...) or split the large files into module
folders along their seams.

Crates buy enforced boundaries and parallel compilation, but the seams were not
yet clean. `agents/` and `background/` keep separate registries behind separate
mutexes (`agents/registry.rs` `reg()`, a `Mutex<Registry>`; `background/mod.rs`
`reg()`, a `Mutex<Registry>` with a `Condvar`), yet they call each other in both
directions, and the correctness of those calls rests on lock rules no type
enforces:

- Where both locks are held, the agents' lock is taken first
  (`background/persist.rs`).
- Code in `background/` that needs the agent layer lets its own lock go first:
  saving `helpers.json` reads `agents::ties_of` after collecting entries, and
  the helper rows read lineage and state after their snapshot.
- Queue authorization runs background → agents without nesting
  (`background/queue.rs` `settle_batch`): a batch is reserved under the
  background lock, each sender of command text is asked
  `agents::command_reach_now` with no lock held, and the background lock is
  taken again to apply the answer only if the reservation still stands (same
  claim, same in-flight ids), since a stop may have come meanwhile.

`pty/` also reaches into both CLIs' launch code and into `agents::register` and
`spawn_is` during a start. Cutting crates along such seams would freeze
today's coupling, and these lock rules, into public APIs and `pub` items.

## Decision

Split by module folder first, one concern per file, keeping visibility as tight
as it was (`pub(super)`, `pub(crate)`):

- `agents/`: `registry.rs`, `owed.rs`, `ask.rs`, `read.rs`, `said.rs`,
  `lost.rs`, `privilege.rs`, `delivery.rs`, `resume.rs`, `answer.rs`, `wait.rs`,
  `list.rs`, `teardown.rs`, `persist.rs`, `spawn/`, `notifier/`;
- `background/`, `library/`, `pty/`, `worktree/`, `activity/`, `power/`,
  `procs/`, `keeper/`, `account_settings/`, `claude/session/`,
  `codex/session/` likewise, each OS's code in a file of its own.

Crates come later, once a boundary has proven itself: a module whose callers
use only a small set of functions and whose state is behind its own API.
[ADR-0010](0010-registry-encapsulation.md) is the first step toward that for
`agents/`.

## Consequences

- Navigation and review get easier now, with no public API to maintain.
- Nothing stops one module reaching into another's internals but review and
  `pub(super)`; the compiler does not enforce the layering yet.
- Build times stay those of one crate.
- Candidates for extraction, in order of readiness: `keeper/` and `procs/`
  (OS process handling, few callers), `library/` (a pass with a narrow command
  surface), `worktree/`; `agents/` only after ADR-0010.
