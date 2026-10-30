# Working in this repo

Rules for anyone (person or agent) changing Luna.

## Plan first
- An agent changes nothing before the plan is agreed. First it lays out what it
  found and what it would do — which files, what changes, what it leaves alone
  and why — and only once the operator has agreed does it make the changes. A
  finding met along the way that calls for more than the agreed plan goes back
  to the operator as a new plan, not straight into the code.

## Where things go
- The repo holds only what is in use. No scratch files, experiments or one-off
  scripts get committed; delete what a change made obsolete (icons, CSS,
  commands, assets) in the same change.
- Never create folders next to the repo or elsewhere on disk for testing
  (no `E:\Projects\Personal\luna-test` and the like). Temporary files go in
  your scratchpad or in `test-build/`.

## Where Luna keeps things
- Luna is to live on one drive: whatever it keeps of its own — settings,
  state, logs, downloads, caches — goes in its data folder, `paths::data_dir`,
  which is the exe's folder wherever that is writable. Nothing goes under the
  user's profile, `%LOCALAPPDATA%` or the system's temp unless the OS or a
  dependency leaves no clean way around it; say why in a comment where it
  happens. The CLIs' own account folders are theirs, not Luna's.

## Building and testing
- There is no local Rust or Node toolchain: everything builds in Docker.
  - `./build-windows.ps1` — the release exe (`-Installer` for the setup).
  - `./test-build.ps1` — the **test build**, `test-build/luna-test.exe`.
  - `./screenshots.ps1 -Out test-build/shots` — headless shots of the UI on
    the demo data in `src/dev/demo.ts`.
- Test only on the test build, never on the Luna in use. It has its own
  identifier and keeps its log, attachments, settings and downloaded CLIs
  beside its exe in `test-build/` (ignored by git), so it runs alongside the
  real one. `./test-build.ps1 -Run -DevCaller ...` also registers a scripted
  MCP caller for exercising Luna tools.
- Anything that writes into account folders (settings, sync) is tried on
  throwaway accounts under `test-build/accounts`, not on real ones.

## Code
- Match the surrounding code: its comment density (comments say why), naming
  and idioms. Comments describe what is there now, not what used to be.
- Cross-platform: Windows is home, but nothing may assume it — gate
  Windows-only behaviour and defaults (e.g. `defaultShell: powershell`).
- Everything in the repo is in English: code, comments, docs. The one
  exception is `README.ru.md`, the Russian twin of `README.md`; change both
  together.

## Both providers
Luna runs Claude Code and Codex side by side, and what one CLI does to a
session the other usually has its own way of doing.
- A problem found, or an improvement made, for one CLI is checked for the
  other: does it have the same behaviour (an exit dialog, a prompt, a cleanup,
  a flag it refuses), and does Luna handle it there too? Look at the other
  CLI's actual binary or docs, not at what it is assumed to do.
- What the check finds goes into the plan (see "Plan first"): what each CLI
  does, what Luna does about it, what would change. Where the other CLI needs
  nothing, the plan says why.

## Agent setup
The repo works the same for every coding agent; nothing in it is written for
one CLI only.
- This file is the only instructions file. Codex reads `AGENTS.md`, and
  Claude Code does too when there is no `CLAUDE.md` (2.1.277 and later). Do
  not add a `CLAUDE.md` or `CLAUDE.local.md` anywhere in the tree: either one
  makes Claude Code read it instead of this file.
- Skills live once, in `.agents/skills/<name>/SKILL.md`, which Codex reads.
  Claude Code reads only `.claude/skills/`, so each skill also gets
  `.claude/skills/<name>` as a symlink to `../../.agents/skills/<name>`,
  committed with it. macOS and Linux check symlinks out as they are; on
  Windows it takes Developer Mode and `git config --global core.symlinks
  true`, or the link arrives as a text file Claude Code does not see.
- A setting only one CLI understands goes in that CLI's project file:
  `.claude/settings.json` or `.codex/config.toml`.
