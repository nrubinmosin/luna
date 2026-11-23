# ADR-0034: Luna supplies a permission mode when the CLI config leaves it unset

- **Status:** Accepted
- **Date:** 2026-11-23

## Context

New chats and helpers used the CLI's own mode when the caller left it
unset. Accounts without permission settings then depended on each CLI's
built-in choices. Luna needs a default it can show before a launch and
report afterward while respecting permission choices already configured.

## Decision

An explicit mode wins. When a launch omits the mode, a setting in the CLI
config Luna reads leaves the choice to the CLI. Otherwise Luna passes
`--permission-mode auto` for Claude Code or Auto-review in
`workspace-write` for Codex. A Codex chat uses `--approve-for-me`; a
helper uses the equivalent reviewer, approval policy, and sandbox
overrides. This supersedes the omitted-mode rule in the
[ADR-0030 amendment](0030-scope-of-0-20.md#amendment-2026-11-20-luna-is-a-runtime-for-the-clis)
and the decision to add no reviewer in
[ADR-0032](0032-codex-helpers-on-app-server.md).

Codex considers `approval_policy`, `sandbox_mode`, `approvals_reviewer`,
and `default_permissions` in every layer Luna reads, including managed
layers and project layers Codex may distrust. This conservative check
prevents Luna from supplying a reviewer over an administrator's approval
policy. F15 takes precedence over Luna's default and CLI delegation. An
unreadable or unsupported Codex layer gets `on-request` with `read-only`.
Claude Code's mode detection reads account, project, and managed settings
files, including sorted `managed-settings.d` drop-ins. An unreadable
file or a non-string `permissions.defaultMode` leaves the mode to Claude
Code. Luna does not read registry or server-managed Claude Code policy.

## Consequences

A chat saves its choice on the first successful start. If Luna supplies
its default or Codex's closed fallback, restarts and resumes reuse those
flags. Managed policy can still take precedence when the CLI interprets
the flags. A CLI-owned chat saves only the choice to delegate, so the
CLI reads its own settings at each start and picks up later edits.
Helpers choose their default at spawn and keep that choice for their
turns.

The provider defaults readers supply the dialog and account details.
`pty/start.rs` and `agents/spawn/background.rs` record the effective
choice. Tests cover explicit and configured modes, the managed layer and
fallback rules, and the chat's save, reload, and resume.
