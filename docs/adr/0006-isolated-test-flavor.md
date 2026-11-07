# ADR-0006: A test flavor isolated from the Luna in use

- **Status:** Accepted
- **Date:** 2026-10-15 (`5f63cbd` introduced `scripts/test-build.ps1` and
  `tauri.test.conf.json`); accounts of its own followed in `857e717`, the
  background window on 2026-11-07

## Context

Luna is developed while the maintainer's own Luna runs real, long agent work on
the same machine. Trying a build must not touch that instance's sessions,
settings, accounts or window, and an agent testing the UI must not take over
the desktop the owner is using.

## Decision

A separate **test build**, `test-build/luna-test.exe`
(`scripts/test-build.ps1`):

- its own app identifier (`src-tauri/tauri.test.conf.json`), no updater, and
  `LUNA_FLAVOR=test`, under which `paths::data_dir` keeps its log, settings,
  attachments and downloaded CLIs beside its exe in `test-build/` (ignored by
  git);
- its default accounts root is `test-build/accounts`; `-Run` refuses a junction
  or symlink there, and a `-DevAccount` outside it;
- `-DevCaller` registers a scripted MCP caller (`LUNA_DEV_TOKEN`,
  `LUNA_DEV_FOLDER`, `LUNA_DEV_ACCOUNT`), so Luna's tools can be exercised
  without a second Luna as the parent;
- `-Background` (implied by `-DevCaller`) keeps its window off screen,
  unfocused and off the taskbar; it is driven only by means aimed at its own
  windows, never by global input;
- the power rule never reaches the OS in the test build or unit tests
  (`power::dry`).

## Consequences

- Real and test instances run side by side; a crash or a bad migration in the
  test build costs nothing.
- Anything that writes into account folders is tried on throwaway accounts.
- The rules are written for agents too (AGENTS.md), since they do most of the
  testing.
