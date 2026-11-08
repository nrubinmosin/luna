# Changelog

All notable changes to Luna are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/) (pre-1.0: a minor version may change
behavior). Releases before this file existed are summarized per minor version
from the git tags; `git log v<a>..v<b>` has the detail.

## [Unreleased]

### Changed

- Release notes are made from this file: a release's notes are its version's
  section, the sections of every version since the last published release, a
  link to the full changes, and the commands and checksums to verify the
  download. The tag's message no longer carries them, and the update chip
  shows the section's opening paragraph.
- Commit messages follow one written-down form (CONTRIBUTING.md, "Commits"):
  a sentence of at most 72 characters saying what the change does, no type
  prefix, a blank line, then why. The commit-msg hook, the gate and CI refuse
  the rest; past commits stay as they are.
- Dependabot opens its grouped update PRs monthly instead of weekly, and leaves a
  new Node or Rust major to a hand-made migration.
- Tauri 2.12.5 and its plugins' latest releases; base64, dirs, getrandom and
  toml_edit moved to versions the rest of the tree already carried, which drops
  dirs 6 and toml_edit 0.22 from the build; vt100 0.18 for the screen a helper's
  output is replayed onto.

### Fixed

- The release workflow failed at attesting the npm SBOM (`Unsupported SBOM
  format`): it was written without a serial number, which the attest action
  requires of a CycloneDX file. Both SBOMs now carry one and are checked for it
  as soon as they are made, and the attestations use `actions/attest` in place
  of the deprecated `actions/attest-sbom`.

## [0.10.0] - 2026-11-08

A security and engineering release. Agent tools now follow least privilege,
Luna's files are private from the moment they are created, and the build, tests
and releases run in CI. Everyday behavior is otherwise unchanged, apart from the
fixes below.

### Security

- A helper session can never hold more privilege than the session that started
  it, and only a session's own helpers accept its approvals and raw keystrokes.
- Text that a CLI would run as a command (a line starting with `/` or `!`)
  reaches a helper, visible or in the background, only when a full-access
  session sends it to its own helper.
- The app window runs under a strict Content Security Policy instead of none.
- Luna's data folder is made private when Luna is installed and checked again at
  every start, and files holding secrets are private from the moment they are
  created.
- A file in the data folder that someone else planted or replaced with a link
  is moved aside instead of being written to.
- On Windows, the install folder and the folders holding secrets get protected
  permissions of their own, and the installer resets only the permissions it
  owns.
- Secrets come from the operating system's secure random generator and are
  compared in constant time, and each chat's hook URLs carry a secret of their
  own.
- Claude Code receives its token for Luna's tools through the environment
  instead of a config file.
- Luna's local listeners cap connections before authentication, parse requests
  strictly and enforce time limits.
- A worktree path containing a line break can no longer forge a worktree
  record, and a Codex worktree is removed only from a Codex account's worktrees
  folder.

### Added

- A threat model and a vulnerability reporting policy (`SECURITY.md`).
- Documentation in `docs/`: a user guide by topic, architecture pages, decision
  records, notes on how each CLI behaves, and a development guide.
- A contributing guide, issue and pull request templates, and this changelog.

### Changed

- Luna is dual-licensed under MIT OR Apache-2.0.
- Releases are built, signed and attested by GitHub Actions from a version tag,
  behind a maintainer's approval, with SBOMs and checksums published beside the
  installer; `docs/releasing.md` explains how to verify a download.
- CI runs on every push on Linux and Windows: formatting, lints, type checks,
  the frontend's tests and build, and the Rust unit tests on both systems.
- Toolchains and build images are pinned to exact versions, and every build
  step's result is checked.
- The Rust code and the frontend are split into smaller modules along their
  natural seams, with no change in behavior.
- Test coverage grows: frontend tests, golden tests of how both CLIs are
  started, and a live suite against the real CLIs on throwaway accounts.
- The repository is written in US English, which a check enforces, and commit
  messages carry no AI attribution, which a hook and CI enforce.

### Fixed

- Worktrees in a folder written with 8.3 short names, such as a temp or profile
  folder under a long user name, are recognized, so helpers return to their
  worktree and Codex chats keep their history.
- Running Luna as the built-in Windows Administrator no longer finds the data
  folder open to others on every start.
- An elevated Luna no longer moves aside the logs and lock files it created
  itself.
- Enter in the account picker acts on the highlighted account, even after the
  list is reordered or that account is removed.
- A line break typed into an answer or note no longer submits it early and sends
  the rest as a separate prompt.
- Text sent to a background helper after its turn was stopped now starts the
  next turn instead of waiting indefinitely.
- A background helper whose queued messages were all refused no longer starts
  an empty turn, and the sender is told that its text was refused.

### Removed

- `ARCHITECTURE.md`, replaced by the architecture pages in `docs/`.
- The Russian README; the repository's text is in English only.

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

[Unreleased]: https://github.com/nrubinmosin/luna/compare/v0.10.0...HEAD
[0.10.0]: https://github.com/nrubinmosin/luna/compare/v0.9.10...v0.10.0
[0.9.0]: https://github.com/nrubinmosin/luna/compare/v0.8.0...v0.9.0
[0.8.0]: https://github.com/nrubinmosin/luna/compare/v0.7.0...v0.8.0
[0.7.0]: https://github.com/nrubinmosin/luna/compare/v0.6.0...v0.7.0
[0.6.0]: https://github.com/nrubinmosin/luna/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/nrubinmosin/luna/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/nrubinmosin/luna/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/nrubinmosin/luna/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/nrubinmosin/luna/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/nrubinmosin/luna/releases/tag/v0.1.0
