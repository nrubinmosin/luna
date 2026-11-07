# Changelog

All notable changes to Luna are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/) (pre-1.0: a minor version may change
behavior). Releases before this file existed are summarized per minor version
from the git tags; `git log v<a>..v<b>` has the detail.

## [Unreleased]

A refactoring campaign on the `refactor/phase-*` branches: no intended change to
user-facing behavior.

### Phase 0: tooling and repository hygiene

- One local gate, `scripts/gate.ps1`, running the same steps as CI; CI split into
  frontend, Linux Rust, Windows build and Windows test jobs, on every branch push.
- Rust and Node toolchains pinned (`rust-toolchain.toml`, `.node-version`), base
  images pinned by digest, every build step's exit code checked.
- Build scripts moved into `scripts/` with shared plumbing in `scripts/lib.ps1`.
- Prettier, ESLint, rustfmt and clippy configured and applied; stricter
  TypeScript checks; Vitest for the frontend.
- The repository is in US English, enforced by `pnpm check:text`.
- Generated schemas untracked, formatting commits skipped by blame, a pre-commit
  hook for skill symlinks.

### Phase 1: security

- A threat model (`docs/security/threat-model.md`) and a vulnerability reporting
  policy (`SECURITY.md`).
- Secrets from the OS's CSPRNG, compared in constant time; per-chat HMAC secrets
  on hook and guard URLs; Claude Code's MCP token passed by environment only.
- Bounded loopback listeners before authentication; the hub's request head parsed
  by `httparse`, with deadlines.
- A least-privilege contract for agent tools: helpers capped at their caller's
  privilege, `allow` and raw keys only on the caller's own helpers, command text
  only from full access.
- Private data folder and secret files from creation; Windows DACLs verified at
  every start; installer hardening.
- A strict Content Security Policy replaces `csp: null`.
- Worktree records read from `git worktree list -z`; a Codex worktree removed
  only from a Codex account's worktrees folder.
- Commit messages carry no AI attribution: commit-msg hook and CI check.

### Phase 2: Rust module structure

- Large files split into module folders along their seams: `agents/` (registry,
  owed, privilege, delivery, spawn, notifier, ...), `background/`, `library/`,
  `pty/`, `worktree/`, `activity/`, `power/`, `procs/`, `keeper/`,
  `account_settings/`, `claude/session/`, `codex/session/`.
- Golden tests of how both CLIs are started, in a pane and headless.

### Phase 3: frontend structure and tests

- IPC split into a transport (`ipc/core.ts`) and one module per domain.
- Styles split into app-wide sheets and the features' own; settings, library and
  the demo fixture split by section.
- Tests for stores, keymap, agent bridge, IPC layer, dialogs and the terminal
  pool.

### Phase 4: test window

- The test build can run its window in the background (`-Background`, implied by
  `-DevCaller`): off screen, unfocused, off the taskbar.

### Phase 5: documentation

- `docs/` with a user guide, architecture pages, ADRs, CLI compatibility notes and
  a development guide; `ARCHITECTURE.md` retired.
- Dual license, MIT OR Apache-2.0; `CONTRIBUTING.md`, this changelog, issue and
  pull request templates.

## [0.9.0] - 2026-11-04

- A folder's skills reach the sessions Luna starts in it, worktrees included.
- A turn that ends on a question to the user shows as waiting.
- 0.9.1–0.9.10 (to 2026-11-06): fixes and refinements.

## [0.8.0] - 2026-11-01

- Helpers survive a core restart, notice loops are broken, read-only Codex with
  network (`read-only-net`), helpers watched without reading them.
- 0.8.2–0.8.3: what a turn leaves running is named and no longer keeps the PC up
  for good; a Linux check.

## [0.7.0] - 2026-10-29

- Skills and instructions of your own, laid out into accounts and folders.
- 0.7.3: background helpers outlive the core; links open in the browser.

## [0.6.0] - 2026-10-15

- The Windows XP look, toned for reading all day; the sidebar slides out from the
  window's edge; Alt+digit peeks a pane.
- 0.6.11: sessions run in a separate core process.
- 0.6.23: background helpers; their questions reach the agent; Codex's sandbox
  works.

## [0.5.0] - 2026-10-15

- The shell redesigned: a task-pane sidebar, a full-width status bar, one icon set.

## [0.4.0] - 2026-09-24

- Agents: a session can spawn, drive and delete other sessions over MCP.
- Keep the PC awake while sessions work, and power off once they are done.

## [0.3.0] - 2026-09-17

- Codex as a second provider.

## [0.2.0] - 2026-08-14

- The installer is released with each version.
- 0.2.16: Luna ships, verifies and updates its own Claude Code binary.

## [0.1.0] - 2026-08-14

- First release: Claude Code sessions in panes, accounts with isolated configs,
  usage limits, worktrees, the updater.

[Unreleased]: https://github.com/nrubinmosin/luna/compare/v0.9.10...HEAD
[0.9.0]: https://github.com/nrubinmosin/luna/compare/v0.8.0...v0.9.0
[0.8.0]: https://github.com/nrubinmosin/luna/compare/v0.7.0...v0.8.0
[0.7.0]: https://github.com/nrubinmosin/luna/compare/v0.6.0...v0.7.0
[0.6.0]: https://github.com/nrubinmosin/luna/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/nrubinmosin/luna/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/nrubinmosin/luna/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/nrubinmosin/luna/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/nrubinmosin/luna/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/nrubinmosin/luna/releases/tag/v0.1.0
