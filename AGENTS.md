# Working in this repo

Rules for anyone (person or agent) changing Luna.

## Plan first
- An agent changes nothing before the plan is agreed. First it lays out what it
  found and what it would do — which files, what changes, what it leaves alone
  and why — and only once the operator has agreed does it make the changes. A
  finding met along the way that calls for more than the agreed plan goes back
  to the operator as a new plan, not straight into the code.
- What a script or a tool can do, the agent does; the operator is asked for
  decisions, not for hands.

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
- Rust builds run in Docker, on the version pinned in
  `src-tauri/rust-toolchain.toml`.
  The scripts take PowerShell 7.4+ (pwsh).
- **The gate a change passes is `ci.yml` on GitHub.** The operator's mirror
  pushes every `slice/*`, `gpui/*`, `core/*` and `iteration/*` branch to
  origin within about a minute of a new commit, and CI runs on each push
  (formatting, lints, the Linux and Windows builds and unit tests,
  the PowerShell checks and the commit-message check). An agent commits with a proper
  message, never pushes (the mirror's job, not the agent's), and reads the
  result with `./scripts/ci-wait.ps1` (or `gh run list` / `gh run view`);
  it never reruns or cancels a run.
- The local scripts below are for the test build, the live tests and the
  narrowest check needed while editing (one test, one lint, one crate
  build), not a full local gate before every commit: every Docker build on
  this PC holds one machine-wide lock, so a full gate here makes every other
  agent's build wait, and CI does the same work in parallel.
- Run `docker` from pwsh, not from Git Bash (the shell of Claude Code's Bash
  tool on Windows). There MSYS rewrites a bind mount like
  `-v /e/Projects/x:/app` into `E:\Projects\x;C:\<Git's folder>\app`, and
  Docker makes an empty folder `E:\Projects\x;C` beside the project, which
  then turns up as a project folder. If it has to be Git Bash, put
  `MSYS_NO_PATHCONV=1` before the command and give the host path with its
  drive letter (`-v E:/Projects/x:/app`).
- The scripts:
  - `./scripts/ci-wait.ps1` — waits for CI on GitHub to finish for a commit
    (HEAD of this branch by default) and prints how it ended: the failed
    jobs, their steps and the tail of their logs. Exits 0 on success, 1 on a
    failed run, 2 on timeout, 3 when `gh` is missing or not logged in.
  - `./scripts/gate.ps1` — the steps CI runs, locally: formatting, lints,
    PowerShell checks and Linux unit tests (`-Windows`
    adds clippy over the Windows build). `-Group` narrows it to one job's
    steps.
  - `./scripts/windows-tests.ps1 -Build -Run` — the unit tests built as
    Windows executables and run on this machine, as CI's Windows job does.
  - `./scripts/build-windows.ps1` — the release exe (`-Installer` for the setup).
  - `./scripts/test-build.ps1` — the **test build**, `test-build/luna-test.exe`.
  - `./scripts/linux-check.ps1` — the crate built for Linux and all its unit tests run
    there, the Unix code included.
  - `./scripts/gpui-shots.ps1` — the native window's demo scenes
    (`src-tauri/crates/window/src/demo.rs`) shot off screen to PNGs; `-Fit` first
    runs the fit test, every scene and popup at small screen sizes, failing on a
    control out of reach (docs/development.md, "Fit on small screens").
  - `./scripts/fuzz.ps1` — the fuzz targets over the transcript readers, on a
    nightly pinned for them alone (docs/testing.md, "Fuzzing").
- Test only on the test build, never on the Luna in use. It has its own
  identifier and keeps its log, attachments, settings and downloaded CLIs
  beside its exe in `test-build/` (ignored by git), so it runs alongside the
  real one. `./scripts/test-build.ps1 -Run -DevCaller ...` also registers a scripted
  MCP caller for exercising Luna tools.
- An agent runs the test build with `-Background` (implied by `-DevCaller`):
  its window stays off screen, unfocused and off the taskbar, and keeps
  working. Drive its UI only by targeted means aimed at the test build's own
  windows (WebDriver on its WebView, messages to its own HWNDs, UI Automation
  patterns on its own elements), never by anything that activates a window.
  The owner's desktop is off limits: no global input (SendInput, keybd_event,
  mouse_event, SendKeys, cursor moves), no SetForegroundWindow, nothing aimed
  at a window that is not the test build's.
- Anything that writes into account folders (settings, sync, helpers run end to
  end) is tried on throwaway accounts under `test-build/accounts`, not on real
  ones, and never through a junction or symlink to one: `scripts/test-build.ps1 -Run`
  refuses links there, and a `-DevAccount` outside it. The test build's
  default accounts root is that folder.
- Before stopping a Luna core or the PC (a restart test, a kill), check
  `luna_list` and `luna_sessions` for work in progress: it loses its turn.

## Code
- Match the surrounding code: its comment density (comments say why), naming
  and idioms. Comments describe what is there now, not what used to be.
- Cross-platform: Windows is home, but nothing may assume it — gate
  Windows-only behavior and defaults (e.g. `defaultShell: powershell`).
- Everything in the repo is in US English: code, comments, UI strings, docs.
  Enforced by `pwsh -File scripts/check-text.ps1`.

## Docs
The docs live in `docs/` (index: `docs/README.md`) and are kept current with
the code, in the same change.
- Behavior a person sees goes in `docs/user-guide/`; how a part works (its
  modules, invariants, failure modes) in `docs/architecture/`; a decision that
  is costly to reverse in a new ADR in `docs/adr/`, from `0000-template.md`.
- What a CLI was seen to do (a flag, a hook, a file format, a quirk) goes in
  `docs/cli-compatibility.md`, with the version it was seen on.
- Toolchains, scripts and CI are in `docs/development.md`; notable changes go
  under [Unreleased] in `CHANGELOG.md`.

## Both providers
Luna runs Claude Code and Codex side by side, and what one CLI does to a
session the other usually has its own way of doing.
- A problem found, or an improvement made, for one CLI is checked for the
  other: does it have the same behavior (an exit dialog, a prompt, a cleanup,
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
- The skills there are the processes too long for a line here: `live-test`
  (the live suite on throwaway accounts), `release` (a version from bump to
  published draft), `cli-upgrade` (a new Claude Code or Codex version),
  `luna-security-review` (a change to a trust boundary, against the threat
  model), `ui-check` (gate, screenshots and test-build smoke for a UI
  change) and `commit-message` (a message that passes the hook and CI,
  and the git moves around one: a refusal, fixups, rewording, reverts).
- Once per clone, `git config core.hooksPath .githooks`: its pre-commit hook
  refuses a commit with anything under `.claude/skills/` that is not a symlink.
- A commit message is a sentence saying what the change does, 72 characters at
  most and no type prefix, then a blank line, then why (CONTRIBUTING.md,
  "Commits"); the commit-msg hook and CI refuse the rest.
- No attribution trailers in commits (`Co-Authored-By:`, the robot emoji,
  "Generated with ..."): authorship is git's author field only. Enforced by
  the commit-msg hook and, over every pushed commit, by CI.
- A setting only one CLI understands goes in that CLI's project file:
  `.claude/settings.json` or `.codex/config.toml`.
