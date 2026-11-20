# Development

Rust builds and tests run in Docker
([ADR-0004](adr/0004-docker-only-toolchain.md)). The scripts in `scripts/` need
PowerShell 7.4+ (`pwsh`) and Docker, on any OS. Testing strategy and suites are
in [testing.md](testing.md); cutting a release is in [releasing.md](releasing.md).

## Toolchains

| Tool | Pinned in |
| --- | --- |
| Rust | `src-tauri/rust-toolchain.toml` and the images' `FROM rust:<version>` |
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
  `fixup!`, `squash!` or `amend!` passes the hook and fails CI. CI checks every
  pushed commit the same way. By hand:
  `sh scripts/check-commit-msg.sh FILE` or `--range A..B`; the check's own tests
  are `scripts/check-commit-msg.test.sh`, run by the scripts job.

## Scripts

The gate a change passes is [CI](#ci) on GitHub, not a run of the scripts
below. The operator's mirror pushes every `slice/*`, `gpui/*`, `core/*` and
`iteration/*` branch to origin within about a minute of a new commit, and
`ci.yml` runs on each push. An agent commits with a proper message, never
pushes, and reads the result with `./scripts/ci-wait.ps1` (or `gh run list` and
`gh run view`). The local scripts are for the test build, the live tests and the
narrowest check needed while editing, not a full gate before every commit: every
Docker build on one PC holds a machine-wide lock (`Use-BuildLock`), so
full gates run one after another there, while CI runs them in parallel.

| Command | What it does |
| --- | --- |
| `./scripts/ci-wait.ps1` | waits for the `ci.yml` run of a commit (`-Sha`, HEAD by default) on a branch (`-Branch`, the current one by default) in the origin repository (`-Repo`), first for it to appear (the mirror may lag), then to finish; `-TimeoutMinutes` (60) and `-PollSeconds` (30). Read-only: it reruns and cancels nothing. Prints the run's id, url and each job's conclusion; on a failure each failed job with its failing steps and the last `-LogLines` (40) lines of its log. Needs `gh`, logged in. Exits 0 on success, 1 on a failed run, 2 on timeout, 3 when `gh` is missing or not logged in or the repository, branch or commit cannot be worked out. Its decisions are tested on sample JSON by `scripts/ci-wait-lib.test.ps1`, a step of the gate |
| `./scripts/gate.ps1` | US English, PowerShell script tests, toolchain pins, Rust formatting and lints, Linux tests and the macOS bundle script tests. Stops at the first failure and reports a summary |
| `./scripts/gate.ps1 -Group scripts` | the PowerShell checks on the host. Other groups are `rust` and `windows` |
| `./scripts/gate.ps1 -Windows` | adds clippy over the Windows build |
| `./scripts/windows-tests.ps1 -Build -Run` | the unit tests of the app and the window crate cross-built as Windows executables (files named by cargo's JSON messages), copied to a folder of their own and run on this machine |
| `./scripts/linux-check.ps1` | the crate built for Linux and all its unit tests run there: the only place the Unix code (`keeper/`, `procs/unix.rs`, parts of `pty/`) compiles. Checkout mounted read-only; caches in named volumes |
| `./scripts/test-build.ps1` | the test build, `test-build/luna-test.exe` (below); `-Test` is the Windows tests |
| `./scripts/build-windows.ps1` | the release exe, `src-tauri/target/x86_64-pc-windows-msvc/release/luna.exe` (self-contained), built with `cargo xwin build` |
| `./scripts/build-windows.ps1 -Installer` | and the unsigned NSIS setup, made by makensis from `src-tauri/windows/installer.nsi`: `…/release/bundle/nsis/Luna_*_x64-setup.exe` ([below](#installer)) |
| `sh scripts/macos-app.sh <exe> <version> <out-dir>` | `Luna.app`, unsigned, from a macOS build of the exe; on a macOS runner (`LUNA_APP_DRY_RUN=1` elsewhere, as `scripts/macos-app.test.sh` in the gate does) |
| `./scripts/gpui-shots.ps1` | the native window's demo scenes as PNGs in `test-build/shots/gpui/` ([below](#native-window-shots)); `-Scenes <part of a name>`, `-NoBuild`; `-Fit` runs the fit check ([below](#fit-on-small-screens)) and shoots each scene at its sizes |
| `./scripts/fuzz.ps1` | the fuzz targets over the transcript readers, built and run in `docker/fuzz.Dockerfile`'s image; `-Seconds`, `-Reproduce <file>`, `-Minimize` (see [testing.md](testing.md#fuzzing)) |
| `./scripts/live-tests.ps1` | the live suite on real CLIs and throwaway accounts (`scripts/test-accounts.ps1`); never run by CI, see [testing.md](testing.md) |
| `./scripts/release.ps1` | bump the version, or tag a release for the workflow (see [releasing.md](releasing.md)) |
| `./scripts/supply-chain.ps1` | the supply-chain checks: cargo-deny and gitleaks ([below](#supply-chain)) |
| `./scripts/third-party-notices.ps1 -Out <file>` | the license texts and source for each crate linked into the Windows exe, the Rust standard library inventory, and the components listed in `third-party-licenses/components.json`. The installer ships the notices and the release publishes them. The script fails when a shipped crate has no license text |

`pwsh -File scripts/check-text.ps1` checks every tracked file for letters outside
the Latin script and British spellings. Pass file paths to check only those files.
A line that must keep its wording carries a `lang-check: allow` comment, either
on that line or alone just above it.

## The test build

Test only on the test build, never on the Luna in use
([ADR-0006](adr/0006-isolated-test-flavor.md)):

```powershell
./scripts/test-build.ps1 -Run                  # build and start it
./scripts/test-build.ps1 -Run -Background      # window off screen, unfocused, off the taskbar
./scripts/test-build.ps1 -Run -DevCaller       # plus a scripted MCP caller (implies -Background)
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
- `-DevCaller` sets `LUNA_DEV_TOKEN` (+ `LUNA_DEV_FOLDER`, `LUNA_DEV_ACCOUNT`),
  registering a caller `dev` whose spawns are top-level chats.
- Drive its UI only by means aimed at its own windows (WebDriver on its WebView,
  messages to its own HWNDs, UI Automation on its own elements). No global input,
  no `SetForegroundWindow`.
- Before stopping a core or the PC, check `luna_session_list` and `luna_session_list` for
  work in progress.

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
image and runs it here. Each scene of `crates/window/src/demo.rs`, in light
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
`close`, one a line, read every 200 ms; `dump` writes the board's state into
Luna's log) and `LUNA_TEST_CLIPBOARD` a JSON file standing in for the
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
leave it alone, and so does the text check). How the window works:
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
popover, menu, drop-down, panel or dialog open as the shots open it, afresh
at each size: the window's first size, 1000×700 and its minimum (1280×800,
1000×700, 900×600), then the laptop screens the WebView window was measured
on, with their display scaling: 1280×720 and 1024×600, 1280×720 at 125% and
150%, and 1024×600 at 150% (683×400 logical pixels). Every element named
with a selector (the crate's `.selector(..)`, `selector.rs`) that is drawn
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
and still after the check; the test fails on a scene that acts and names
nothing. A drop-down whose face is wheeled out of view, even in part, must
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
pushes (`slice/*`, `gpui/*`, `core/*`, `iteration/*`) is checked by it within
minutes of a commit, and `./scripts/ci-wait.ps1` reads the outcome:

| Job | Runs |
| --- | --- |
| `scripts` | `gate.ps1 -Group scripts` for the US English check and PowerShell tests, then the commit-message check over the pushed range |
| `rust-linux` | `gate.ps1 -Group rust` in `docker/linux-check.Dockerfile`'s image |
| `windows-build` | `gate.ps1 -Group windows` and `windows-tests.ps1 -Build`, uploading the test executables; then `third-party-notices.ps1`, as the release makes it on a Linux runner |
| `windows-tests` | runs those executables on a `windows-2025` runner |

`.github/workflows/gpui.yml` builds the app with its native window on a
`macos-15` runner (Apple silicon), on pushes to `gpui/**` branches and by
hand: the dependency graph checked (no gpui-pre-windows, CoreText's
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

`./scripts/supply-chain.ps1` runs, each in Docker on a pinned tool:

| Check | What fails it |
| --- | --- |
| `tests` | the tests of the checks themselves (`scripts/supply-chain-lib.test.ps1`): the ignore rules, `deny.toml` read as TOML, gitleaks' output kept free of secrets |
| `deny` | cargo-deny 0.20.2 (release binary, SHA-256 pinned in the script) with `src-tauri/deny.toml`: a RustSec advisory anywhere in the tree (vulnerable, unmaintained, unsound), a license outside the allowlist, a wildcard requirement, a crate from anywhere but crates.io. Duplicate versions and yanked crates are warnings |
| `secrets` | gitleaks 8.30.1 (image by digest) over the history reachable from `HEAD`, or `-Range <base>..<head>`. A finding prints its rule, file, line and commit only, never the text it matched: gitleaks' own redaction covers each finding's secret, not the rest of its line |

```powershell
./scripts/supply-chain.ps1                     # all three; exit 1 if any fails
./scripts/supply-chain.ps1 -Check deny,secrets   # some of them
```

They need the network (the advisory databases, the registries), and their answer
changes with no change to the repo, so the gate does not run them and stays
hermetic. In CI, `security.yml` runs each check as a job of its own (`gitleaks`
over a pull request's commits, or master's whole history on schedule and by
hand) and CodeQL over the workflows and Rust, build-mode
none. CodeQL's analysis jobs hold no write permission, since Rust's extraction
loads the tree through cargo, which can run its build scripts and proc macros; a
separate `codeql-upload` job, which runs no project code, uploads the results to
the Security tab. The runner is Linux, so the Windows-only modules are parsed
without macro expansion or the semantic analysis the rest gets.

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
  `…/release/bundle/nsis/Luna_<version>_x64-setup.exe`, beside the third-party
  notices it carries, where the release scripts look for it. Before makensis
  runs, the script refuses a `luna.exe` whose PE machine is not AMD64 (the
  setup itself is an x86 program, as every NSIS setup is). makensis runs
  with `-WX`: any warning fails the build, and `installer.nsi` refuses a
  makensis whose strings (`NSIS_MAX_STRLEN`) are under 1024 units, which
  the updater sizes the restart's arguments to. It downloads nothing. A
  pre-release version (`x.y.z-pre`) names the setup in full and gives its
  version resource `x.y.z.0`. The script is adapted from Tauri's template,
  which its header credits and `third-party-licenses/` carries the license
  of.

It is never run on the development machine: an install writes the
registry and the Start menu. The script is checked by compiling it, and by
reading it against Tauri's template; the setup it makes, by the updater's
own check, which a Windows test exe runs on the file named in
`LUNA_CHECK_SETUP`
(`--ignored --exact desktop::updater::tests::the_setup_in_luna_check_setup_passes_for_one`).

## Releases

Releases are built, signed and attested by `.github/workflows/release.yml`, on a
`v*` tag that `scripts/release.ps1` makes:

```powershell
./scripts/release.ps1 -Bump 1.2.3   # write the version into its four files
# commit on a branch, merge it into master through a pull request, then on master:
./scripts/release.ps1               # check the merged HEAD, tag it v<version> (annotated)
git push origin v1.2.3              # starts the release workflow
```

- The script refuses to tag if the version files (`package.json`,
  `tauri.conf.json`, `Cargo.toml`, `Cargo.lock`) disagree, the tree is dirty, HEAD
  is ahead of `origin/master`, or the tag exists.
- The workflow verifies versions, tag and a green CI run on the commit
  (`release-verify.ps1`), builds without any secret (`build-windows.ps1`,
  `release-bundle.ps1`), signs in a job of its own in the `release` environment
  (`release-sign.ps1`), writes the Cargo SBOM (`sbom.ps1`) and attestations, and leaves a
  **draft** release: setup, `.sig`, `latest.json`,
  a CycloneDX SBOM and `SHA256SUMS`. `latest.json`'s `core_fingerprint` comes from
  `core-fingerprint.txt`, which `build.rs` leaves beside the exe, so no runner has
  to run the Windows binary. The logic these scripts share is
  `scripts/release-lib.ps1`, tested in the gate.
- No bare exe is published: the updater installs into Luna's install folder
  ([ADR-0011](adr/0011-private-data-dir-beside-the-exe.md)).
- The updater key is a private minisign key kept outside the repo; its public half
  is in `tauri.conf.json`. No build takes it: `build-windows.ps1 -Installer` makes
  the setup unsigned so dependency build code runs outside the sign job.
  `release-sign.ps1` signs it afterwards with the key from the file `-UpdaterKey`
  or `LUNA_UPDATER_KEY` names, or from `TAURI_SIGNING_PRIVATE_KEY` (as the
  workflow has it), running only the Tauri CLI's `signer sign`, held to the
  integrity `pnpm-lock.yaml` pins, in the pinned Node image without network; it
  checks the signature against the public key (`updater-signature.mjs`) and puts
  it into `latest.json`.
- `./scripts/release.ps1 -Local` still cuts a release on this machine when the
  workflow cannot.

The whole flow, the one-time GitHub setup, key rotation and how to verify a release
are in [releasing.md](releasing.md).

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
3. **Expose it.** A command the core answers is mapped in `corecmd.rs` and
   listed in `CORE_CALL` in `src/ipc/core.ts`; a window-only command is
   registered in `lib.rs`. Events go through `emit.rs`.
4. **Frontend.** Add the call and its DTO to the domain module in `src/ipc/`,
   state to the feature's store, UI under `src/features/<feature>/`;
   provider-specific UI under `features/providers/<cli>/`. Add a Vitest test and,
   if it shows in screenshots, demo data in `src/dev/demo/`.
5. **Agents.** If agents should reach it, add or extend a tool in `mcp.rs` /
   `agents/`, keep to Luna's own admission (who reaches which session; [threat model](security/threat-model.md), section 3),
   update `src-tauri/src/luna_guide.md` and the threat model's matrix.
6. **Check.** CI on the pushed commit (`./scripts/ci-wait.ps1`; or
   `./scripts/gate.ps1` yourself where there is no mirror),
   `./scripts/linux-check.ps1` for Unix code, and the test build for anything a
   person sees. Update the user guide and the
   architecture page the change touches, and record a consequential decision as
   an ADR ([template](adr/0000-template.md)).
