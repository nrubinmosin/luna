# ADR-0014: The core credential stays a bearer token behind a read deny

- **Status:** Accepted
- **Date:** 2026-11-08

## Context

A window finds the core through `<data>/core/core.json`, which carries the
core's port and its bearer token (`coreproc.rs` `write_discovery`); the first
frame on the loopback port must hold that token (`coreproc.rs` `serve`), and
then `corecmd.rs` grants every command: sessions of either CLI in any mode,
input into any session, account settings, power, worktree removal. A CLI
session confined by its sandbox that can read the file and reach the port
leaves its confinement (threat model F3). An independent audit rated it High.

What each CLI does about the file, as seen on Codex 0.167.0 and Claude Code
2.1.318:

- Codex's read-only policies read the whole disk, and `read-only-net`,
  Luna's own value, has the network. A permission profile can deny a folder.
  On Linux that holds under `:read-only` and `:workspace`. But `:workspace`
  drops what an account's `[sandbox_workspace_write]` adds (its roots, its
  network), so a profile cannot stand in for `workspace-write` without
  changing what the session may do.
- On Windows Luna runs Codex's unelevated sandbox. Its token is restricted
  for writes only: a command reads what the user can, including `core/`
  under its private DACL, and its `read-only` connects to loopback. A deny in
  a profile makes that sandbox refuse every command ("requires the elevated
  Windows sandbox backend"). The elevated backend needs an administrator's
  setup, which Luna does not do.
- Claude Code has no OS sandbox under Luna (none on native Windows). Its
  permission rules can deny the file tools a path; Bash, where a mode runs it
  unasked, reads anything.

A council of two independent reviews weighed accepting the risk, a read deny,
and a new protocol.

## Decision

The core keeps its bearer token in `core.json`, and every session Luna starts
is denied the folder wherever its CLI can enforce that:

- Claude Code: every settings file (`claude/launch.rs` `claude_settings`)
  carries `permissions.deny` `Read(//<core>/**)`, the path spelled as Claude
  Code spells it (`claude_read_deny`).
- Codex off Windows: `read-only` and `read-only-net`, interactive and
  `codex exec`, run under a profile of Luna's, named afresh at every start,
  that extends `:read-only` and denies `core/` (`codex/launch.rs`
  `denies_core`, `profile_args`; `pty/start.rs` `codex_core_deny`).
- Codex `workspace-write` (and `auto-review`) and every Codex sandbox on
  Windows get no deny, for the reasons above. They stay open, as F3 says.

Considered and deferred, **protocol v2**: no credential on disk. The core
alone creates windows and hands each one an inherited channel (a socketpair,
an anonymous pipe); discovery carries only the core's identity and a
rendezvous that can do nothing but ask for a window; the unauthenticated
`--stop-core` gives way to a request on that rendezvous; a core speaking v1 is
recognized and not trusted as protected.

## Consequences

- Small and local: launch arguments and a settings entry, covered by unit
  and golden tests, and a live test (`live.rs`
  `codex_sandboxes_reach_the_core_folder_as_the_threat_model_says`) that
  checks each sandbox's reach with Codex's own runner and fails when either
  side changes.
- Only the read-only Codex sandboxes off Windows are closed by the OS.
  Claude's deny is a rule its Bash does not obey, and any unconfined process
  of the user's still reads the file (R3).
- Protocol v2 is deferred because it rebuilds the window–core handshake, the
  installer's stop and every core start, while the deny covers the case the
  audit put first (a read-only sandbox with the network) on Linux and macOS
  at the cost of an argument. It comes back when a supported sandbox is found
  able to read `core/` despite the deny, or when any core surface is exposed
  beyond loopback. Codex's unelevated Windows sandbox meets the first
  condition today, as does `workspace-write` everywhere: whether that brings
  v2 forward is an open decision, recorded in the threat model's F3.
