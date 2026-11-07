# ADR-0013: Luna ships and updates its own copies of the CLIs

- **Status:** Accepted
- **Date:** 2026-08-23 (`a71182a`, "Ship a private Claude CLI"); Codex followed
  with the second provider

## Context

A global `claude` or `codex` install updates itself whenever it likes. A binary
replaced under a running session, a version that changes its on-disk formats
overnight, or a missing install on a fresh machine all break Luna, which reads
those formats ([ADR-0001](0001-session-state-lives-in-the-cli.md)).

## Decision

`cli.rs` keeps Luna's own copies in `<data>/claude-cli/versions/<ver>/` and
`<data>/codex-cli/versions/<ver>/`, with a `current` pointer; each provider
supplies a `Source` (`claude/cli.rs`, `codex/cli.rs`). Versions are never
overwritten (a running exe cannot be replaced on Windows): a new one goes next
to it, the pointer switches, and old ones are swept once no session holds them.
Downloads are verified by sha256 from the vendor's own manifest. Claude Code
runs with `DISABLE_AUTOUPDATER=1`; Codex accounts get
`check_for_update_on_startup = false`. Until a copy exists Luna falls back to
`PATH`. Updates are checked every six hours.

## Consequences

- Nothing depends on a global install, and only Luna moves either binary.
- Luna follows CLI releases closely; a format change shows up in its own update
  cycle, recorded in [cli-compatibility.md](../cli-compatibility.md).
- The checksum comes from the same origin as the binary, so a compromised
  vendor channel is not detected (finding F10 in the
  [threat model](../security/threat-model.md)).
