# ADR-0011: Luna's data lives in one private folder beside the exe

- **Status:** Accepted
- **Date:** 2026-08-23 (`a71182a`, the CLI copies under "Luna's own data dir");
  made private at creation and verified at every start on 2026-11-07
  (`f1b3f60`, `233990a`, finding F4)

## Context

Luna keeps settings, logs, attachments, downloaded CLIs, the core's token,
hook settings files and agent records. Scattering them over the user profile,
`%LOCALAPPDATA%` and the system temp makes a portable install impossible and
leaves secrets in folders other users or processes may read. The operator
wants Luna to live on one drive.

## Decision

Everything Luna keeps goes in `paths::data_dir`: the exe's folder wherever it is
writable, else `%LOCALAPPDATA%\luna` (on Unix a `luna-<uid>` folder at mode
0700, refused if it belongs to someone else). The test build keeps everything
in `test-build/` ([ADR-0006](0006-isolated-test-flavor.md)).

- On Windows the folder is verified private at every core start
  (`protect_data_dir`): a protected DACL where it is Luna's alone; the
  installer hook (`src-tauri/windows/hooks.nsh`) prepares the install folder.
- Folders holding secrets (`core/`, `hooks/`, `mcp/`) are 0700 and token files
  0600 from creation; a secret folder that is a link or another user's is
  refused.
- Only where an OS or dependency leaves no clean way around it does anything go
  elsewhere, with a comment saying why. The CLIs' own account folders are
  theirs, not Luna's.

## Consequences

- Install anywhere, on any drive; uninstalling removes everything.
- The updater must install into the folder Luna runs from, so no bare exe is
  released ([development](../development.md#releases)).
- Agents are kept out of this folder: no helper is rooted in or above it
  ([ADR-0007](0007-least-privilege-for-agent-tools.md)).
