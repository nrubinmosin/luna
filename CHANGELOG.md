# Changelog

All notable changes to Luna are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/) (pre-1.0: a minor version may change
behavior). Releases before this file existed are summarized per minor version
from the git tags; `git log v<a>..v<b>` has the detail.

## [Unreleased]

### Security

- No CLI runs with administrator rights. Luna started as administrator starts
  its core, and so every session and helper, with the desktop's ordinary
  rights and the user's own environment, not the elevated one's; where there
  are none (no Explorer, another account's credentials, User Account Control
  off) it starts no session and says to restart it normally. The core refuses
  sessions and helpers whenever it runs elevated, and a core an older Luna
  left running elevated is replaced or, with sessions, restarted on the
  banner's button; one whose rights cannot be read counts as elevated. An
  elevated core lets an ordinary window in only to stop it, past the check
  that otherwise turns away a client of lower integrity; a sandboxed one is
  still refused. Root is refused the same way on Linux and macOS, and the
  live tests refuse to run from an elevated terminal.

## [0.10.1] - 2026-11-08

A security and reliability release; it also carries the changes of 0.10.0,
which was never published. Claude Code downloads are checked against
Anthropic's signed release, sessions are kept away from the key to Luna's
core, an interrupted CLI install is no longer mistaken for a finished one,
shutting the PC down or restarting it when done works again, and the
updater's signing key is kept out of the release build.

### Security

- Dependencies with published vulnerabilities are updated: rustls 0.23.46 and
  rustls-webpki 0.103.16 (TLS handshake fixes), webpki-roots 1.0.11 (distrusted
  emSign roots removed), zip 9 under the updater (tauri-plugin-updater
  2.14.1) and tokio 1.53.3 (an unsound `AsyncFd` constructor); among the build
  tools, Vite 7.3.7 and source-map-js 1.2.2.
- Every pull request, every change to master and a weekly run check the
  dependencies for advisories, licenses and sources (cargo-deny, `pnpm audit`),
  the history for committed secrets (gitleaks), and the code and workflows with
  CodeQL. `scripts/supply-chain.ps1` runs the same checks locally.
- The updater's signing key no longer reaches the release build. `tauri build`
  runs every dependency's build scripts, proc-macros and frontend tooling, and
  any of them could have read the key it was handed; the installer is now
  built unsigned and signed afterwards in a job of its own, which runs only the
  Tauri CLI pinned by `pnpm-lock.yaml`, without network, and checks the
  signature against the updater's public key before publishing it. The release
  still takes one approval, now given to the signing job. `release.ps1 -Local`
  signs the same way, builds an export of the commit rather than the checkout
  it signs from, and no longer runs the built exe to ask its core fingerprint.
  A key rotation's bridge release is checked against the old public key, given
  as `UPDATER_TRUSTED_PUBKEY`.
- Claude Code's release manifest is verified against its detached OpenPGP
  signature and Anthropic's release key, pinned in Luna, before the exe's
  checksum from it is trusted: whoever serves the download bucket can no
  longer hand Luna an exe of their own with a matching manifest. Codex is
  still checked only against the checksums of its own release.
- The signed manifest must name the version it was fetched for, and no Claude
  Code version below the highest signed one Luna has accepted (or below
  2.1.319 on a fresh install) is installed, so an old signed release cannot be
  replayed under the unsigned `latest` pointer. A signature is held to its
  signed creation and expiration times, and the release key to its expiry
  where the signature gives one.
- Sessions are denied the folder that holds the key to Luna's core, wherever
  their CLI can enforce it: Claude Code's file tools everywhere, and Codex's
  `read-only` and `read-only-net` sandboxes on Linux and macOS. A Claude Code
  session whose deny cannot be written does not start, rather than run without
  it.
- On Windows, where a Codex sandbox can still read that file, the core instead
  refuses the connection: it checks which process is at the other end of each
  loopback connection and turns away one running in a CLI sandbox (a restricted
  or contained token, or a lower integrity level) before it can present the
  key. A normal window is unaffected. Codex's `workspace-write` on Linux and
  macOS remains able to read the file; the threat model (F3) says why.

### Added

- The installer puts `THIRD-PARTY-NOTICES.txt` beside Luna, and each release
  publishes it too: the license texts of every crate and npm package Luna is
  built from, and where their source is; the Rust standard library's own
  inventory; and what the build tools and the installer add: Vite's helpers,
  NSIS and its Tauri plug-in, the WebView2 loader and the Visual C++ runtime.

### Changed

- `luna_power` disarms with the action `cancel`; `off`, its old name, still
  works.
- Release notes are made from this file: a release's notes are its version's
  section, the sections of every version since the last published release, a
  link to the full changes, and the commands and checksums to verify the
  download. The tag's message no longer carries them, and the update chip
  shows the section's opening paragraph.
- Commit messages follow one written-down form (CONTRIBUTING.md, "Commits"):
  a sentence of at most 72 characters saying what the change does, no type
  prefix, a blank line, then why. The commit-msg hook, the gate and CI refuse
  the rest; past commits stay as they are. A refusal says to commit again from
  the message, not to amend, and `amend!` commits are let through to be
  squashed, as `fixup!` ones are.
- Dependabot opens its grouped update PRs monthly instead of weekly, and leaves a
  new Node or Rust major to a hand-made migration.
- Tauri 2.12.5 and its plugins' latest releases; base64, dirs, getrandom and
  toml_edit moved to versions the rest of the tree already carried, which drops
  dirs 6 and toml_edit 0.22 from the build; vt100 0.18 for the screen a helper's
  output is replayed onto.

### Fixed

- A CLI install cut short (Luna killed or the PC off while Codex's package was
  unpacked) could be taken for a finished one on the next check: the executable
  being there was enough, so Codex ran without its sandbox helpers or from a
  truncated file. Both CLIs are now fetched into a staging folder, checked file
  by file against what their release publishes (Claude Code: sha256 and size;
  Codex: the package's sha256, then each file's size), given a completion
  record, and moved into place with one rename under a name no folder has, so a
  version a session is running is never touched. Only a version with its record
  counts as installed; ones installed before this are fetched again once. The
  files, the record and (on Unix) the folders are flushed to disk before the
  version is named current, and once Luna has installed this way a version
  without its record is never run. Sweeping old versions deletes nothing
  through a link or junction below them. Codex's package is unpacked with caps
  on its entries, its bytes as the tar reader writes them, each metadata record
  and the decompressed stream, and a link entry or PAX size override in it is
  refused.
- Shut down and restart when done turn the PC off again. Luna asks Windows
  itself, with the shutdown privilege turned on, instead of running
  `shutdown.exe`, which had been failing without a word; a refusal is logged
  with its Windows error.
- Windows is asked before any session is closed, so a refusal leaves every chat
  running, and a shutdown Windows records as canceled after the chats were
  closed brings them back where they were instead of leaving dead panes. One
  whose fate Windows does not tell leaves the chats closed and says how to bring
  them back, rather than reopening them under a shutdown still on its way.
- Windows' notice during the grace says that open programs are closed by force
  and unsaved work in them is lost, and so does the user guide.
- Shutdown winds Luna down the way restart does, so a chat parked mid-turn is
  not saved as finished.
- A restart that Windows refuses, or that is canceled, no longer leaves Luna set
  to start at the next sign-in.
- The release workflow failed at attesting the npm SBOM (`Unsupported SBOM
  format`): it was written without a serial number, which the attest action
  requires of a CycloneDX file. Both SBOMs now carry one and are checked for it
  as soon as they are made, and the attestations use `actions/attest` in place
  of the deprecated `actions/attest-sbom`.
- The release and test builds failed: `tauri build` refuses a Tauri crate and its
  npm package on different minor versions, and the notification plugin's crate
  had moved to 2.8 while `@tauri-apps/plugin-notification` stayed at 2.7. The
  package is on 2.8 now, and the gate checks every such pair in the lockfiles
  (`pnpm check:tauri`), so a bump to one side alone fails there.

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

[Unreleased]: https://github.com/nrubinmosin/luna/compare/v0.10.1...HEAD
[0.10.1]: https://github.com/nrubinmosin/luna/compare/v0.10.0...v0.10.1
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
