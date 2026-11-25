# Development

The Windows and Linux build scripts use Docker and the pinned Rust toolchain
([ADR-0004](adr/0004-docker-only-toolchain.md)). They need PowerShell 7.4+
(`pwsh`). Windows test executables run on a Windows host, and GitHub builds
and tests macOS on Apple silicon. See [Testing](testing.md) and
[Releasing](releasing.md).

## Toolchains

| Tool | Pinned in |
| --- | --- |
| Rust | `src-tauri/rust-toolchain.toml`, the images' `FROM rust:<version>` and `rust-version` in `src-tauri/Cargo.toml` and `src-tauri/crates/window/Cargo.toml`; the gate fails when the toolchain file, the app's manifest and the images disagree |
| cargo-xwin, NSIS | `docker/windows-build.Dockerfile` |
| Rust nightly, cargo-fuzz (fuzzing only) | `docker/fuzz.Dockerfile` and `src-tauri/fuzz/rust-toolchain.toml`; `scripts/fuzz.ps1` fails when the two disagree |

The gate fails when the Rust pins disagree. Each base image has a tag and
digest. Build images are tagged by a hash of their Dockerfile.
`scripts/lib.ps1` also provides the build lock, build stamp and shared
container arguments.

## First clone

```powershell
git config core.hooksPath .githooks   # pre-commit and commit-msg hooks
```

- The pre-commit hook refuses anything under `.claude/skills/` that is not a
  symlink. Skills live in `.agents/skills/<name>/`, with `.claude/skills/<name>`
  a symlink to it; on Windows that needs Developer Mode and
  `git config --global core.symlinks true`.
- The commit-msg hook checks the message against the rules in
  [CONTRIBUTING.md](../CONTRIBUTING.md#commits): a subject of at most 72
  characters that says what the change does (a capital, a digit or a path first;
  no type prefix, no trailing period), a blank line, a body wrapped at 72, and no
  AI attribution (`Co-Authored-By:`, the robot emoji, "Generated with ...").
  Merges, reverts and the release bump `Luna X.Y.Z` keep git's own form; a
  `fixup!`, `squash!` or `amend!` passes the hook and fails CI. CI checks the
  stored messages of each push or pull request the same way; off `master` it
  leaves out the commits `origin/master` already holds. By hand:
  `sh scripts/check-commit-msg.sh FILE` or `--range A..B`; the check's own tests
  are `scripts/check-commit-msg.test.sh`, run in `gate.ps1 -Group scripts`.

## Scripts

The gate a change passes is [CI](#ci) on GitHub, not a run of the scripts
below. The operator's mirror pushes `slice/*`, `gpui/*`, `core/*`,
`iteration/*` and `stage6/*` branches to origin within about a minute of a
new commit. `ci.yml` runs on each push. An agent commits with a proper message,
leaves the push to the mirror, and reads the result with `./scripts/ci-wait.ps1`
(or `gh run list` and
`gh run view`). The local scripts are for the test build, the live tests and the
narrowest check needed while editing, not a full gate before every commit: every
Docker build on one PC holds a machine-wide lock (`Use-BuildLock`), so
full gates run one after another there, while CI runs them in parallel.

| Command | What it does |
| --- | --- |
| `./scripts/ci-wait.ps1` | waits for the `ci.yml` run of a commit (`-Sha`, HEAD by default) on a branch (`-Branch`, the current one by default) in the origin repository (`-Repo`), first for it to appear (the mirror may lag), then to finish; `-TimeoutMinutes` (60) and `-PollSeconds` (30). Read-only: it reruns and cancels nothing. Prints the run's id, url and each job's conclusion; on a failure each failed job with its failing steps and the last `-LogLines` (40) lines of its log. Needs `gh`, logged in. Exits 0 on success, 1 on a failed run, 2 on timeout, 3 when `gh` is missing or not logged in, the repository, branch or commit cannot be worked out, or `gh` fails five times in a row while it polls. Its decisions are tested on sample JSON by `scripts/ci-wait-lib.test.ps1`, a step of the gate |
| `./scripts/gate.ps1` | US English, PowerShell script tests, toolchain pins, Rust formatting and lints, Linux tests and the macOS bundle script tests. Stops at the first failure and reports a summary |
| `./scripts/gate.ps1 -Group scripts` | the PowerShell checks and commit-message tests on the host. Other groups are `rust` and `windows`; `-Group scripts,rust` runs several |
| `./scripts/check-changes.ps1` | validates changelog fragments and requires one for product changes on feature branches |
| `./scripts/gate.ps1 -Windows` | adds clippy over the Windows build |
| `./scripts/windows-tests.ps1 -Build -Run` | the unit tests of the app and the window crate cross-built as Windows executables (files named by cargo's JSON messages), copied to a folder of their own and run on this machine. `-Build` and `-Run` also work separately. `-Dir` selects the staging folder, which defaults to `test-build/windows-tests`. `-Build` writes only into a folder directly in `test-build/`. `-Filter checkout::tests,codex::trust::tests` runs tests matching either name fragment. Pass comma-separated fragments with `pwsh -File` or a string array from a PowerShell session. A filtered run fails if no tests pass. |
| `./scripts/linux-check.ps1` | the crate built for Linux and all its unit tests run there, the Unix code (`keeper/`, `procs/unix.rs`, parts of `pty/`) included, which the Windows build leaves out (`gate.ps1 -Group rust` and the macOS workflow compile it too). Checkout mounted read-only; caches in named volumes |
| `./scripts/test-build.ps1` | the test build, `test-build/rm-luna-test.exe` (below); `-Test` is the Windows tests |
| `./scripts/build-windows.ps1` | the release exe, `src-tauri/target/x86_64-pc-windows-msvc/release/luna.exe` (self-contained), built with `cargo xwin build`; `-Source <folder>` builds another tree and writes the same paths there (the local release builds a git export that way) |
| `./scripts/build-windows.ps1 -Installer` | and the unsigned NSIS setup, made by makensis from `src-tauri/windows/installer.nsi`: `…/release/bundle/nsis/rm-luna_*_x64-setup.exe` ([below](#installer)) |
| `sh scripts/macos-app.sh <exe> <version> <out-dir>` | `Luna.app` from a macOS build of the exe, with an ad-hoc seal (`codesign -s -`, no identity) where `codesign` is there; on a Mac (`LUNA_APP_DRY_RUN=1` elsewhere, as `scripts/macos-app.test.sh` in the gate does). No workflow runs it yet |
| `./scripts/gpui-shots.ps1` | the native window's demo scenes as PNGs in `test-build/shots/gpui/` ([below](#native-window-shots)); `-Scenes <part of a name>`, `-NoBuild`; `-Scale 100,125,175` shoots each at those window scales; `-Fit` runs the fit check ([below](#fit-on-small-screens)) and shoots each scene at its sizes into `fit/` under the output folder; `-Out` names another output folder |
| `./scripts/fuzz.ps1` | the fuzz targets over the transcript readers, built and run in `docker/fuzz.Dockerfile`'s image; `-Seconds`, `-Reproduce <file>`, `-Minimize` (see [testing.md](testing.md#fuzzing)) |
| `./scripts/live-tests.ps1` | the live suite on real CLIs and throwaway accounts (`scripts/test-accounts.ps1`); never run by CI, see [testing.md](testing.md). It runs the test build's managed CLIs, found through their `state.json` record, or else the matching `.exe` on `PATH` (`scripts/live-tests-lib.ps1`, tested on made-up trees by `scripts/live-tests-lib.test.ps1`, a step of the gate) |
| `./scripts/release.ps1` | bump the version and collect changelog fragments, or tag a release for the workflow (see [releasing.md](releasing.md)) |
| `./scripts/supply-chain.ps1` | the supply-chain checks: cargo-deny and gitleaks ([below](#supply-chain)) |
| `./scripts/third-party-notices.ps1 -Out <file>` | the license texts and source for each crate linked into the Windows exe, the Rust standard library inventory, and the components listed in `third-party-licenses/components.json`. The installer ships the notices and the release publishes them. The script fails when a shipped crate has no license text |

Write release notes in `changes/<slug>.md` with a lowercase slug and a
`###` heading such as `### Fixed`, followed by bullets. Use `### None` with
a line explaining why when a product change needs no note. The scripts group
runs `check-changes.ps1` to validate [the fragment format](../changes/README.md)
and keep `## [Unreleased]` empty. A pull request uses its target branch as
the base. For a push or local check, the checker chooses the closest merge
base with `origin/master`, `origin/iteration/*`, `origin/stage6/*`,
`origin/gpui/*`, or `origin/core/*`, excluding the branch being checked.
Product changes need a fragment added in the diff or the branch's own
fragment already present in the tree. Name that fragment after the branch
without its prefix. The rule covers the app, native UI, installer, shipped
assets, vendored crates, `Cargo.lock`, and Cargo's product dependency,
feature, and release profile tables. Dedicated test files are exempt. On
`master` and versioned release branches, only the content checks run.

`pwsh -File scripts/check-text.ps1` checks every tracked text file for letters
outside the Latin script and British spellings, and that known text types are
UTF-8; generated, vendored and license files, binaries and links are passed
over. Pass file paths to check only those files.
A line that must keep its wording carries a `lang-check: allow` comment, either
on that line or alone just above it.

## The test build

Test only on the test build, never on the Luna in use
([ADR-0006](adr/0006-isolated-test-flavor.md)):

```powershell
./scripts/test-build.ps1 -Run                  # build and start it
./scripts/test-build.ps1 -Run -Background      # window off screen, unfocused, off the taskbar
./scripts/test-build.ps1 -Run -DevCaller -DevFolder <project folder> -DevAccount test-build\accounts\<provider>\<name>
                                               # plus a scripted MCP caller (implies -Background unless -Foreground)
./scripts/test-build.ps1 -Run -NoBuild         # start the last build
./scripts/test-build.ps1 -Test                 # the Rust tests, built for Windows, run here
```

- Its own identifier and name (`rm.luna.test`, "Luna Test":
  `src-tauri/src/identity.rs`, from `LUNA_FLAVOR=test`), no updater; log,
  settings and downloaded CLIs live beside its exe in `test-build/`
  (ignored by git), so it runs alongside the real one.
- Its default accounts root is `test-build/accounts`. Anything that writes into
  account folders is tried on throwaway accounts there, never through a junction
  or symlink to a real one: `-Run` refuses links there, and a `-DevAccount`
  outside it.
- `scripts/test-accounts.ps1 clone` leaves the source login behind. A person
  gives the throwaway account its own fresh sign-in under
  `test-build/accounts`. See the Claude Code and Codex sign-in steps in
  [Testing](testing.md#accounts).
- `-DevCaller` sets `LUNA_DEV_TOKEN` (+ `LUNA_DEV_FOLDER`, `LUNA_DEV_ACCOUNT`),
  registering a caller `dev` whose spawns are top-level chats.
- Drive its UI only with the [test driver file](#native-window-shots) or messages
  to its own window handles. No global input and no `SetForegroundWindow`.
- Before stopping a core or the PC, check `luna_session_list` for work in progress.

## Screenshots

The screenshots are the native window's demo scenes, shot as below. Shooting
a running Luna would shoot whatever is open in it; the scenes retake every
picture from the same invented chats and accounts. The README's and the user
guide's pictures in `docs/screenshots/` are copied from a run's light and
dark scenes: `panes.png` from `chats-4-light.png`, `dark.png` from
`chats-2-dark.png` and `new-chat.png` from `dialog-new-chat-light.png`.

### Native window shots

`./scripts/gpui-shots.ps1` shoots the native window: it builds
`test-build/luna-window-shots.exe` (the window crate's `shots` feature, in the
`shots` profile: release without its link-time optimization) in the Windows
image and runs it here. Each scene of `src-tauri/crates/window/src/demo.rs`, in light
and dark, is opened over a fake host past the right edge of the displays,
never activated, and rendered in the app (`Window::render_to_image`) to
`test-build/shots/gpui/<scene>-<theme>.png`; a frame with nothing drawn
fails the run. It needs gpui's test support, so the app's exe never carries
it. The macOS workflow (`.github/workflows/gpui.yml`) runs the same harness.

`-Fit` runs the fit check ([below](#fit-on-small-screens)) in the Linux
image, then shoots every scene at each of its sizes into
`test-build/shots/gpui/fit/<scene>-<theme>-<w>x<h>.png`, to look at.

The same exe runs the terminal's burst bench:
`test-build/luna-window-shots.exe --bench 60 test-build/bench.json` feeds a
terminal pane 60 s of synthetic bursts through the seam a session's output
takes and writes its frame times, latencies, lock waits and the grid's
checksum against a headless replay (architecture/native-window.md,
"Screenshots").

The test build's board shows the chats of its own `window/chats.json` (or,
on its first start, those its own data dir's 0.19 WebView store held); their
accounts must be under `test-build/accounts`. For a smoke run that may use
no global input and must not touch the clipboard, `LUNA_TEST_DRIVE` names a
file of lines the window acts on (`keys ctrl-c enter`, `text words`,
`focus 2`, `row 3`, `put 2 3`, `swap 1 2`, `unseat 1`, `toboard`, `dump`,
`close`, one a line, read every 200 ms once its newline is written; `dump`
writes the board's state into Luna's log, and `status`, `terms`, `panels` and
`dialogs` the status bar's, the terminals', the panel's and the dialog's; the
status bar, orphan line, notices, dialogs and panels take commands of their
own, all listed in `src-tauri/crates/window/src/drive.rs`) and `LUNA_TEST_CLIPBOARD` a JSON file standing in for the
clipboard (`{"text": ...}`, `{"files": [...]}` or `{"image": true}`; an image
or nothing types `[test clipboard: image key]` rather than press the CLI's
key, which would read the real clipboard). They are compiled into the test
build alone (the `luna_test` cfg both build scripts set for
`LUNA_FLAVOR=test`); a release has none of them.

## The native window

The window crate, `src-tauri/crates/window` (package `luna-window`), is a
member of the workspace `src-tauri/Cargo.toml` makes, on its lockfile, and
builds with the app: every script above that builds the exe builds it. Its
pins are exact (`gpui-pre`, `gpui-pre-platform` and `gpui-component`, one
snapshot), and GPUI's Windows crate is vendored and patched
(`src-tauri/vendor/`, PATCHES.md says what and why; `cargo fmt` and clippy
leave it alone, and so does the text check). Its four patches, each marked
`luna patch`: shaders compiled at runtime (so a Linux cross-build of a
release builds), unattended windows (the test build's and the shots'
background mode), frames on demand (no redraw on every vsync), and frames
through input (a window presents on every vsync for a second after the
person's input, for variable-refresh monitors). The vendored crate is
outside the workspace, so CI does not run its unit tests; `cargo xwin test
-p gpui-pre-windows --lib --target x86_64-pc-windows-msvc --no-run` in the
Windows build image builds them, to be run on Windows. How the window works:
[architecture/native-window.md](architecture/native-window.md).

### The window's font

Cascadia Mono, the window's one font, is carried in the exe from
`src-tauri/crates/window/assets/fonts/`: the static TrueType Regular and Bold
of Microsoft's release 2407.24 (`ttf/static/` of `CascadiaCode-2407.24.zip`,
https://github.com/microsoft/cascadia-code/releases/tag/v2407.24), with its
SIL Open Font License 1.1 beside them (`OFL.txt`, the repository's `LICENSE`
at that tag) and in `third-party-licenses/installer/cascadia-mono/`.

| File | SHA-256 |
| --- | --- |
| `CascadiaCode-2407.24.zip` | `e67a68ee3386db63f48b9054bd196ea752bc6a4ebb4df35adce6733da50c8474` |
| `CascadiaMono-Regular.ttf` | `06520d032ec274fa5040b22c6f4a1d829081b24ba40b2da56dae89bf10c7b481` |
| `CascadiaMono-Bold.ttf` | `b22cb603ed23cac36e8444846e1841caca21719a5e852780a8d37ae7a49b0a36` |
| `OFL.txt` | `51882cd3cdba4e16f220f44ddb08a635c38c44ea6e0975db2574f4be6f958238` |

A new release of the font replaces both files and the table, and moves the
entry in `third-party-licenses/components.json`.

## Fit on small screens

The window crate's fit test (`src-tauri/crates/window/src/fit_tests.rs`)
opens every light scene of the shots (the dark ones change no size) with its
popover, menu, drop-down, panel or dialog open as the shots open it. It tests
the default size (1280×800), 1000×700, and the minimum (900×600). Laptop
coverage includes 1280×720 and 1024×600 at 100%, 1280×720 at 125% and 150%,
and 1024×600 at 150% (683×400 logical pixels). Every element named with a selector (the crate's `.selector(..)`, `selector.rs`) that is drawn
must be in reach. It fails on a control that is

- past the window's edge, or squeezed to nothing;
- cut off by a frame that clips it (a dialog's or panel's edge, a row's
  `overflow_hidden`) while that frame is on screen: in the crate's tests each
  selector draws a probe that notes the content mask it is painted under,
  which gpui's `debug_bounds` do not give;
- inside a scroller that the wheel does not bring it into view in: the test
  wheels the scroller round a control cut off toward it, as a person scrolls,
  in steps no longer than the room the view has beside it, and fails only if
  that does not show it whole, or makes it go;
- covered: a hit test at its middle does not reach the probe's hitbox, as
  something drawn over it takes the mouse. Under a popup or a modal that
  is meant: in a scene that opens a frame, only what is inside the frame is
  held to this, in one that opens a menu only what the menu must show.

A scene that opens anything names what it must then show, and the frame
that must fit where it opened with nothing scrolled (`OPENS`), at every size
(the status bar's "..." list only below 800 px, the only place it is) and
still after the check; the test fails on a scene that acts and names
nothing. The title bar's and the status bar's controls must be drawn in every
scene, but for Clear board and Auto-tile below 800 px, where they are rows of
the title bar's Menu, drawn in the scene that opens it. A drop-down whose face is wheeled out of view, even in part, must
close (`SCROLL_AWAY`). What is out of reach today is listed exactly, by
scene, size, selector and how (`KNOWN`), with why: anything else fails the
test, and so does an entry in reach, so the list only shrinks as the window
is fixed. Button labels stay on one line (`whitespace_nowrap` in both button
faces), so a row too narrow for its buttons shows as one past its edge.

Selectors are recorded only in debug builds, so CI's Linux and macOS jobs run
it with the crate's other tests (the Windows tests are release builds);
`./scripts/gpui-shots.ps1 -Fit` runs it alone, the crate built anew from the
checkout (the Linux target volume is every checkout's), and shoots the
scenes at its sizes. A screen it cannot reach gets a scene in `demo.rs` and
an `OPENS` entry.

## CI

`.github/workflows/ci.yml` runs on every branch push, every pull request and by
hand, with the same scripts. It is the gate: a branch the operator's mirror
pushes (`slice/*`, `gpui/*`, `core/*`, `iteration/*`, `stage6/*`) is checked by it
within minutes of a commit, and `./scripts/ci-wait.ps1` reads the outcome:

| Job | Runs |
| --- | --- |
| `scripts` | `gate.ps1 -Group scripts` for the US English and changelog fragment checks and PowerShell tests, then the commit-message check over the pushed range |
| `rust-linux` | `gate.ps1 -Group rust` in `docker/linux-check.Dockerfile`'s image |
| `windows-build` | `gate.ps1 -Group windows`, `windows-tests.ps1 -Build` and `third-party-notices.ps1` (as the release makes it on a Linux runner), then uploads the test executables |
| `windows-tests` | runs those executables on a `windows-2025` runner |

`.github/workflows/gpui.yml` builds the app with its native window on a
`macos-15` runner (Apple silicon), on pushes to `gpui/**` and `stage6/0.20`
and by hand: the dependency graph checked (no gpui-pre-windows, CoreText's
zed-font-kit present), the window crate's tests, the app's unit tests
(reported, not yet required) and the window's shots, uploaded as one artifact.
macOS is built and tested there only ([ADR-0030](adr/0030-scope-of-0-20.md)).

`.github/workflows/fuzz.yml` runs each fuzz target for five minutes on Mondays
and by hand, keeping only what one finds as an artifact (see
[testing.md](testing.md#fuzzing)).

`.github/workflows/security.yml` runs the supply-chain checks and CodeQL on
every pull request, every push to master, weekly and by hand
([Supply chain](#supply-chain)).

`.github/workflows/release.yml` runs only on `v*` tags and by hand
([Releases](#releases)). Actions are pinned by commit and images by tag and digest.
The project updates the Actions, images, crates and toolchains by hand through
the gate ([ADR-0029](adr/0029-dependencies-move-by-hand.md)). Cargo-deny and
Dependabot alerts cover crate advisories. For Actions pinned by SHA and images
pinned by digest, review the release notes for fixes when moving the pin.

## Supply chain

`./scripts/supply-chain.ps1` runs these, `deny` and `secrets` in Docker on a pinned tool, `tests` in PowerShell on the host:

| Check | What fails it |
| --- | --- |
| `tests` | the tests of the checks themselves (`scripts/supply-chain-lib.test.ps1`): the ignore rules, `deny.toml` read as TOML, gitleaks' output kept free of secrets |
| `deny` | cargo-deny 0.20.2 (release binary, SHA-256 pinned in the script) with `src-tauri/deny.toml`: a RustSec advisory anywhere in the tree (vulnerable, unmaintained, unsound), a license outside the allowlist, a wildcard requirement, a crate from anywhere but crates.io. Duplicate versions and yanked crates are warnings |
| `secrets` | gitleaks 8.30.1 (image by digest) over the history reachable from `HEAD`, or `-Range <base>..<head>`, passing over the findings `.gitleaksignore` lists, compared field by field. A finding prints its rule, file, line and commit and its fingerprint, never the text it matched: gitleaks' own redaction covers each finding's secret, not the rest of its line |

```powershell
./scripts/supply-chain.ps1                     # all three; exit 1 if any fails
./scripts/supply-chain.ps1 -Check deny,secrets   # some of them
```

They need the network (the advisory databases, the registries), and their answer
changes with no change to the repo, so the gate does not run them and stays
hermetic. In CI, `security.yml` runs each check as a job of its own (`gitleaks`
over a pull request's commits or what a push to master brought, and the
whole history reachable from the commit on schedule, by hand, or for a push
with no old tip to compare with) and CodeQL over the workflows and Rust, build-mode
none. CodeQL's analysis jobs hold no write permission, since Rust's extraction
loads the tree through cargo, which can run its build scripts and proc macros; a
separate `codeql-upload` job, which runs no project code, uploads the results to
the Security tab. The runner is Linux, so the Windows-only modules are parsed
without macro expansion or the semantic analysis the rest gets.

A gitleaks finding that is no secret (a made-up token in a test, say) is
listed in `.gitleaksignore` at the repository's root by its exact
fingerprint, `<commit>:<file>:<rule>:<line>` with the commit in full, as the
failed check prints it, under a comment saying why it is no secret. The script
refuses any other form. gitleaks is not handed the file: it reports every
finding, and the script passes over only those whose commit, file, rule and
line each equal an entry's, and fails on the rest. gitleaks' own matching
would not do: it keeps its commit-free fingerprints (`<file>:<rule>:<line>`,
which hold in every commit) in the same table as the full ones, so a later
commit's file named `<listed commit>:<listed file>` would pass under an entry.
An entry covers that one finding in that one commit and nothing a later commit
adds (a second secret the same rule finds on the same line of that commit
passes with it). A plain `gitleaks git` run in the repository by hand still
reads the file the way gitleaks does. A pull request whose history holds such
a finding is merged with a merge commit: a squash or a rebase gives its
commits new ids, and the entries then name nothing.

The file is read from the tree being checked, so in a pull request it is the
pull request's own, and a pull request can add a secret and the entry that
passes it over together; the check cannot tell. A new entry is therefore for
the reviewer to check: its commit, file and line hold no real secret. A real
secret is rotated and taken out of the history instead.

An advisory with no fix to take yet is ignored only with a reason and an
expiry, which the script enforces; it fails once the date has passed:

- a crate: an entry in `deny.toml`'s `[advisories] ignore`,
  `{ id = "RUSTSEC-...", reason = "... Expires YYYY-MM-DD" }`, whose reason
  names who pulls the crate in and why it cannot go. The file is read as TOML,
  so an entry in any form cargo-deny takes (a bare id included) is held to this;
A reason of a few words, or only whitespace, is refused. An ignore goes in through a pull request like any change, with the advisory and
why it does not reach Luna in its description; three months is the usual
expiry. The licenses `deny.toml` allows are the ones the tree needs, each
unusual one justified there; a new license in the tree is decided there, not
allowed in bulk.

## Installer

`./scripts/build-windows.ps1 -Installer` builds the setup
([desktop services](architecture/desktop-services.md#installer)) with
makensis, in the Windows build image (NSIS 3.11), over
`src-tauri/windows/installer.nsi`, which includes `hooks.nsh`. The exe is
built with `cargo xwin build`; the setup lands at
  `…/release/bundle/nsis/rm-luna_<version>_x64-setup.exe`, where the release
  scripts look for it, carrying the third-party notices generated first into
  `src-tauri/target/THIRD-PARTY-NOTICES.txt`. Before makensis
  runs, the script refuses a `luna.exe` whose PE machine is not AMD64 (the
  setup itself is an x86 program, as every NSIS setup is). makensis runs
  with `-WX`: any warning fails the build, and `installer.nsi` refuses a
  makensis whose strings (`NSIS_MAX_STRLEN`) are under 1024 units, which
  the updater sizes the restart's arguments to. It downloads nothing. A
  pre-release version (`x.y.z-pre`) names the setup in full and gives its
  version resource `x.y.z.0`. The script is adapted from Tauri's template,
  which its header credits and `third-party-licenses/` carries the license
  of. Cargo's `luna.exe` is installed as `rm-luna.exe`; the setup's names
  are those of Luna's identifier, `rm.luna` (the uninstall key, the key
  that keeps the install folder) and `rm-luna` (the default folder), not
  those 0.19 to 0.20 had, so 0.21 is installed fresh
  ([updates](user-guide/updates.md#switching-from-020)).

It is never run on the development machine: an install writes the
registry and the Start menu. The script is checked by compiling it, and by
reading it against Tauri's template; the setup it makes, by the updater's
own check, which a Windows test exe runs on the file named in
`LUNA_CHECK_SETUP`
(`--ignored --exact desktop::updater::tests::the_setup_in_luna_check_setup_passes_for_one`).

## Releases

The tagged workflow builds the unsigned Windows setup, signs it in a separate
job, and stages a draft release with the setup, signature, updater manifest,
Cargo SBOM and checksums. The updater and the release scripts read the
endpoint and public key from `src-tauri/updater.json`.

See [releasing.md](releasing.md) for version bumps, tagging, signing and
verification.

## Adding a feature end to end

Example: a new core command the UI calls, for both CLIs.

1. **Plan.** Write down what each CLI does today and what changes for each
   (AGENTS.md, "Both providers"). If it depends on a CLI behavior, check the
   binary or its docs and add the observation to
   [cli-compatibility.md](cli-compatibility.md).
2. **Rust.** Put provider-specific logic in `claude/` and `codex/`, shared
   mechanics where they already live ([module map](architecture/overview.md#module-map)).
   Keep launch builders pure; extend the launch golden tests in `pty/start.rs`
   or `background/command.rs` if arguments change.
3. **Expose it.** Add a core command to `corecmd::COMMANDS` on its lane and
   map it in `corecmd.rs`. Add a window-local command to the host dispatch in
   `native.rs`. Events go through `emit.rs`. Async Rust work uses Luna's
   tokio runtime (`runtime.rs`).
4. **Window.** Use the `Host` interface in `src-tauri/crates/window/src/host.rs`
   from the owning model or view. Keep DTOs with their domain and extend Rust
   tests beside the code. For a new visible surface, add a demo scene and
   exercise it in the window crate's fit test.
5. **Agents.** If agents should reach it, add or extend a tool in
   `mcp/tools.rs` (its schema) and `mcp/dispatch.rs` (its call), with the
   session's side in `agents/`, keep to Luna's own admission (who reaches which
   session; [threat model](security/threat-model.md), section 3), and update
   `src-tauri/src/luna_guide.md` and the threat model's matrix (section 5).
6. **Check.** CI on the pushed commit (`./scripts/ci-wait.ps1`; or
   `./scripts/gate.ps1` yourself where there is no mirror),
   `./scripts/linux-check.ps1` for Unix code, and the test build for anything a
   person sees. Update the user guide and the
   architecture page the change touches, and record a consequential decision as
   an ADR ([template](adr/0000-template.md)).
