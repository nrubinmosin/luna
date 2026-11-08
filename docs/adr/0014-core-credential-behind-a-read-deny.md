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
- Codex `workspace-write` (and `auto-review`) get no deny, for the reasons
  above: the read deny is not the Windows measure, and off Windows
  `workspace-write` stays open.

On Windows, where no deny can be given, the core closes the gap at the port
instead of at the file. On each loopback connection, before the hello, it
finds the process that owns the connection's client end
(`GetExtendedTcpTable`, the client endpoint matched exactly, IPv4 and IPv6)
and refuses it when that process's token is restricted, an AppContainer, or of
a lower integrity than the core's, and when the owner cannot be found — fail
closed (`coreproc.rs` `serve`, `peer.rs` `verdict`). The unelevated sandbox's
token is restricted for `read-only` and `workspace-write` alike (measured: its
write access is an ACL grant on the workspace, not a looser token), so a
sandboxed Codex is turned away whatever its mode, while a window and the test
build — a plain token of the core's integrity — are let through. It relies on
Luna never running Codex elevated, so a sandboxed peer is never above the
core. An equivalent peer check is not taken off Windows: Codex's Linux sandbox
is Landlock, the same uid and namespaces as the core, so a connecting process
cannot be told from the window there; macOS is untried.

Considered and deferred, **protocol v2**: no credential on disk. The core
alone creates windows and hands each one an inherited channel (a socketpair,
an anonymous pipe); discovery carries only the core's identity and a
rendezvous that can do nothing but ask for a window; the unauthenticated
`--stop-core` gives way to a request on that rendezvous; a core speaking v1 is
recognized and not trusted as protected.

## Consequences

- Small and local: launch arguments, a settings entry, and a peer check in the
  core listener. Covered by unit and golden tests, the peer decision through a
  pure function fed the facts a connection would carry (`peer.rs` `verdict`),
  and two live tests (`live.rs`): one checks each Codex sandbox's reach of
  `core/` with Codex's own runner; the other that a sandboxed Codex connecting
  to a test port is refused while a plain process is let in. Each fails when
  its side changes.
- Windows is closed by the OS (the sandbox's token betrays it at the port);
  the read-only Codex sandboxes off Windows are closed by the OS (the deny);
  the rest are not. Claude's deny is a rule its Bash does not obey; Codex
  `workspace-write` off Windows has no deny; any unconfined process of the
  user's still reads the file and connects (R3).
- Protocol v2 is deferred because it rebuilds the window–core handshake, the
  installer's stop and every core start, while the read deny and the Windows
  peer check together close the case the audit put first — a read-only sandbox
  with the network — on every platform Luna is tested on, at the cost of an
  argument and a per-connection lookup. It comes back when a supported sandbox
  is found able to both read `core/` and connect unrefused (Codex
  `workspace-write` off Windows is the nearest residual), or when any core
  surface is exposed beyond loopback. That decision is recorded in the threat
  model's F3.
