---
name: cli-upgrade
description: Bring Luna up to a new Claude Code or Codex CLI version — find what changed against the actual binary and its release notes (flags, hooks, headless stream events, transcript and rollout formats, the screen dialogs Luna parses, config keys), check the other CLI for the same thing, update fixtures and golden tests deliberately, record it in docs/cli-compatibility.md and run the live suite. Use it whenever a new claude or codex version is out or was just downloaded, a CLI update broke something in Luna (a dialog not recognized, a helper that never finishes, a missing title or transcript, a refused flag), or someone asks whether Luna supports CLI version X, even if they only say "codex 0.170 is out" or "claude updated and helpers hang".
---

# CLI upgrade

Luna installs on its own the newest version on the known-good list compiled
into each release (`src-tauri/cli-known-good.json`), and on the `latest`
channel each new release once it passes an offline probe of its flags
(ADR-0019). So a new version reaches most users only once this process has
put it on the list, and reaches users on `latest` within hours whether or not
Luna is ready for it. Luna depends on much more than flags: it reads the
CLI's stream, transcripts, rollouts and even its screen, and each of those is
pinned by a test against what one version did. The job is to find every place
the new version differs, decide for each whether Luna must change, and leave
the tests, and the list, saying the new truth.

## 1. Get the facts from the real thing

- **The binary.** The test build's managed copy is the one to look at:
  `test-build/claude-cli/versions/<v>/claude.exe`,
  `test-build/codex-cli/versions/<v>/bin/codex.exe` (`current` names the
  version). The test build installs the known-good version like any Luna;
  to get the new one, put that CLI on `latest` (Settings → CLIs, or the word
  `latest` in `test-build/<cli>-cli/channel` before it starts) and start it
  with `-Background`. A version that fails the offline probe is marked bad
  in `bad.json` with why: that is already a finding.
  Diff the `--help` of every command Luna launches (the launch files below
  say which: `claude`, `codex`, `codex exec` and so on) between the old and
  the new version; keep both outputs under `test-build/` as evidence.
- **The release notes.** Claude Code: `CHANGELOG.md` in
  github.com/anthropics/claude-code. Codex: github.com/openai/codex releases.
  Read every entry since the last version Luna was checked against
  (`docs/cli-compatibility.md`).
- **What Luna claims now.** Grep for the old version number in code comments
  and tests (`rg '2\.1\.3[0-9]{2}'`, `rg '0\.1[0-9]{2}\.0'`): each hit is a
  statement "as version X does it" to re-check.

## 2. Check each surface Luna depends on

| Surface | Where Luna has it | Pinned by |
|---|---|---|
| Launch flags, env, config overrides | `claude/launch.rs`, `codex/launch.rs`, `pty/start.rs`, `background/command.rs`, `codex/config.rs` | golden launch tests (`golden.rs`); the offline probe's flag table (`cli/probe.rs` `COMMANDS`), held to every golden argument list and to the help fixtures in `cli/testdata/` |
| Hooks and their payloads | Claude `--settings` hooks (`pty/start.rs`, `hub.rs`), Codex hooks and their trust (`codex/hooks.rs`) | hub tests |
| Headless stream events | `background/events.rs` | recorded events in its tests |
| Transcripts / rollouts | `claude/session/` (`transcript.rs`, `ending.rs`, `titles.rs`), `codex/session/` (`read.rs`, `rollouts.rs`, `scan.rs`) | fixtures "as version X writes it" |
| Screen: dialogs, lists, composer, title | `pty/screen.rs`, `agents/ask.rs` | screen fixtures |
| Trust prompts, defaults, models, limits | `*/trust.rs`, `*/defaults.rs`, `*/models.rs`, `*/limits.rs` | unit tests |
| Version gates | `checks.rs` | unit tests |
| Download and layout | `claude/cli.rs`, `codex/cli.rs`, `cli.rs` | unit tests |

To see a format for real, produce it on a throwaway account
(`test-build/accounts`, see the `live-test` skill) through the test build or
`claude -p` / `codex exec` with that account's `CLAUDE_CONFIG_DIR` /
`CODEX_HOME`, never in a real account. Screen changes need a test-build chat
on the new version; read its screen through Luna (the dev caller's
`luna_read`), not by looking at a window.

## 3. Both providers

For every change found in one CLI, look at the other's binary or docs for the
same behavior (AGENTS.md, "Both providers"), and write down for each: what it
does, what Luna does about it, what changes. Where the other needs nothing,
say why.

## 4. Plan, then change

The findings go to the person as a plan before code changes (AGENTS.md, "Plan
first"). Then:

- **Fixtures**: add the new version's sample beside the old one rather than
  overwriting it while users may still run the old version; name the version
  in the fixture's comment, as the existing ones do. Drop the old only when
  Luna stops supporting it.
- **Golden tests**: a golden diff is a review item, not noise. Change the
  expected value only for a flag change you meant, and say in the commit why
  each line moved. A golden that fails for a reason you did not intend is a
  bug.
- **Version-specific docs**: `docs/security/threat-model.md` §6 names the
  versions screen detection is known to work on; `ARCHITECTURE.md` notes
  version behaviors. Update what you re-verified.

## 5. Record and prove

- Update `docs/cli-compatibility.md`: each observation under its CLI with the
  version it was seen on, what Luna does about it, and the version now checked.
- `./scripts/gate.ps1`, then `./scripts/windows-tests.ps1 -Build -Run`.
- The live suite on the new version (`live-test` skill): it is the only level
  that sees a CLI's real stream, MCP client and flags.
- Once all three pass, add the version to the known-good list: its CLI's
  `knownGood` in `src-tauri/cli-known-good.json`, in the same change as the
  rest. That is what lets users on the default channel have it, with the next
  Luna release. Raise `minSafe` only when an older version is found unsafe (a
  security fix in the CLI), and say why in the commit: nothing below it runs
  anywhere, rollbacks included. Where the new version's `--help` differs from
  the newest fixture, add its own (`<cli>-<ver>-…-help.txt` in
  `src-tauri/src/cli/testdata/`) beside the old ones, and point the probe's
  test at it. A version not checked this way stays off the list, whatever
  else changed.

## Report

Per CLI: old → new version; each change found with its source (help diff,
changelog line, observed output); what Luna changed or why nothing; tests and
goldens touched and why; gate, Windows tests and live results; what went
onto the known-good list (or why not), and `minSafe` if it moved.
