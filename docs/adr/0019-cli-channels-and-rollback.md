# ADR-0019: CLI channels and rollback

- **Status:** Accepted
- **Date:** 2026-11-09

## Context

Luna installs each CLI's newest release within six hours of it coming out
([ADR-0013](0013-own-cli-binaries.md)), whether or not Luna has been checked
against it, and Luna depends on far more than a CLI's flags: its stream, its
transcripts and its screen ([ADR-0001](0001-session-state-lives-in-the-cli.md)).
A release that breaks one of those reached every chat and helper at once, the
sweep then deleted every version but the current one, so there was nothing to
go back to, and on Unix it could delete a running Codex session's folder.
Claude Code's floor against an unsigned `latest` pointer rose as each signed
manifest was taken, before its version ever ran, and Codex had none.

The options for which versions run were: always the newest (as before);
Anthropic's own `stable` channel (Claude Code only); a list of checked versions
that Luna publishes as a signed feed; or such a list compiled into each Luna
release. For qualifying a version: an offline probe of the binary, a real
headless turn on a dedicated account (quota, a sign-in), or watching the first
sessions. For a version that misbehaves: roll back on its own, or say so and
let a person decide.

## Decision

- **Channels, per provider.** `known-good`, the default, installs the newest
  version on the list compiled into the Luna release
  (`src-tauri/cli-known-good.json`, `cli/known_good.rs`), fetched by its own
  version; `latest` follows the CLI's release pointer, one click away in
  Settings → CLIs (`cli/choice.rs`). The list also names a `minSafe` per
  provider: nothing older is installed, made current or gone back to. There
  is no separate feed: a security fix in a CLI reaches users as a Luna patch
  release that lists it, which goes through the existing signed, attested and
  gated release path; a feed would need the updater's signing key outside the
  `release` environment.
- **Qualification.** Before a new version becomes current, an offline probe
  runs it in a scratch home (`cli/probe.rs`): `--version` must name it, each
  command's `--help` must list every flag Luna passes (a table the golden
  launch tests hold to every argument list they pin), and Codex must load its
  configuration with Luna's hook and MCP overrides. One that fails is marked
  bad and swept. Then a canary watches its first three sessions
  (`cli/canary.rs`); two that exit with an error within half a minute raise a
  warning: a desktop notice, a status-bar chip and "Roll back" in Settings.
  Nothing rolls back on its own.
- **Rollback.** The previous version is always kept, and so is every version a
  process of Luna's runs from: each holds a shared lock on
  `<root>/in-use/<dir>` until it exits, and the sweep takes a folder only
  under the exclusive one (`cli/in_use.rs`). "Use" makes any installed version
  current and holds it there until a channel is chosen again; "Mark bad" keeps
  a version from being installed or made current by a pass.
- **The floor rises when a version becomes current**, not when it is fetched
  (`cli.rs` `flip_in`), for both CLIs. Nothing below it is downloaded or taken
  from the pointer. Going back below it by hand, or by the known-good channel
  to a listed version still installed, is allowed down to `minSafe`, once
  every file matches the sha256 its install record kept; Codex records now
  keep one per file (as unpacked, held to the tarball's checksum), where they
  kept sizes alone.
- **Marks by hand go both ways** (amended 2026-11-12). "Mark good" on an
  installed version off the list adds it, on this machine, to what the
  known-good channel picks from (`good.json`), as a `chosen` version under the
  same floor and `minSafe`; "Clear bad" and "Unmark good" undo either mark,
  installed or not. The canary also says when a version got through: three
  watched sessions past their first half minute, none failed. Neither mark
  nor the canary's word is a Luna release's check, and Settings keeps them
  apart (known-good, marked good, first 3 sessions ok). Weighed: no local
  marks, "Use" alone holding a version the person trusts; it keeps the
  version from following a later list, which marking does not.
- **MCP** gets the version a session's CLI started from (`luna_session_get`,
  `cliVersion`), read-only: changing versions is the person's.

## Consequences

- A CLI release reaches users only once someone has checked it and a Luna
  release lists it, unless they chose `latest` or marked it good themselves
  after running it. A version marked good is one the person vouches for on
  this machine, not one Luna's gate and live suite passed. Each version checked goes onto
  the list as a step of the cli-upgrade skill; a Luna that is not updated
  stays on its list's versions.
- On the known-good channel the release server cannot choose the version at
  all, only withhold it. A Luna whose CLI had moved past its list (on
  `latest`, or before channels existed) stays where it is rather than
  download a listed version below its floor.
- The probe is coarse: it catches a flag gone or a config refused, not a
  changed stream or screen; the canary catches a CLI that cannot run a
  session, not one that runs it wrong. A helper stopped by hand within half a
  minute counts as a failure; the warning only ever asks.
- Disk: the previous version and any still in use stay, about 200 MB for
  Claude Code and 600 MB for Codex each.
- New state at each provider's root: `previous`, `floor`, `channel`,
  `bad.json`, `good.json` and `marks.lock`, `pinned`, `canary.json` and its
  lock, `in-use/`. A same-user process that can write `good.json` could
  already choose `latest` or hold any installed version; it gains no reach,
  and the floor and `minSafe` still hold against it.
