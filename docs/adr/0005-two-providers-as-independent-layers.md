# ADR-0005: Claude Code and Codex are two independent layers

- **Status:** Accepted
- **Date:** 2026-09-17 (`ce0e0b5`, "Codex as a second provider")

## Context

Adding Codex beside Claude Code invited a common "provider" abstraction: one
permission mode, one model list, one limits shape. The two CLIs do not agree on
those concepts. Claude Code has permission modes and effort levels; Codex has
approval policy, sandbox mode, reasoning effort per model, and a catalog of its
own. A shared vocabulary would mistranslate one of them, and every new CLI
feature would have to be forced through it.

## Decision

Two layers, in Rust and on the frontend alike: `src-tauri/src/claude/` and
`src-tauri/src/codex/`, `src/features/providers/claude/` and
`src/features/providers/codex/`. Each describes its CLI in that CLI's own
words: settings, defaults, limits, trust, models, launch arguments, reading the
session from disk. `Provider` (`provider.rs`) is a tag and nothing more.

What is shared is only mechanics: the pty, finding and sweeping worktrees, the
accounts folder, and downloading a CLI binary (`cli.rs`, each provider giving
its `Source`).

## Consequences

- Codex does not know "permission mode", Claude Code does not know "sandbox" or
  "rollout"; a Codex chat reads Codex's `config.toml` and shows Codex's own
  rate-limit windows.
- Some code is parallel rather than shared (two `defaults.rs`, two `launch.rs`,
  two `session/` readers). Golden tests pin both launch paths
  ([pty and launch](../architecture/pty-and-launch.md)).
- Every change for one CLI is checked against the other (AGENTS.md, "Both
  providers"); the outcome is recorded per CLI in
  [cli-compatibility.md](../cli-compatibility.md).
