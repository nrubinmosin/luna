# Development

There is no local Rust or Node toolchain: everything builds and tests in Docker
([ADR-0004](adr/0004-docker-only-toolchain.md)). The scripts in `scripts/` need
PowerShell 7.4+ (`pwsh`) and Docker, on any OS. Testing strategy and suites are
in [testing.md](testing.md); cutting a release is in [releasing.md](releasing.md).

## Toolchains

| Tool | Pinned in |
| --- | --- |
| Rust | `src-tauri/rust-toolchain.toml` and the images' `FROM rust:<version>` |
| Node | `.node-version` and the images' `FROM node:<version>` |
| pnpm | `packageManager` in `package.json` (via corepack) |
| cargo-xwin, NSIS | `docker/windows-build.Dockerfile` |

The gate fails when the pins disagree. Every `FROM` names its image by tag and
digest; the gate's frontend steps run in the Windows image's Node stage, so one
pin serves both. Images are tagged by a hash of what they are built from
(`scripts/lib.ps1`), which also holds the shared plumbing: the
one-build-at-a-time mutex, the build stamp, the docker runs.

## First clone

```powershell
git config core.hooksPath .githooks   # pre-commit and commit-msg hooks
./scripts/gate.ps1 -Group windows     # also writes src-tauri/gen/schemas/
```

- `src-tauri/gen/schemas/` is not tracked: tauri-build writes it on every build,
  and `capabilities/*.json` point their `$schema` at it.
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
  `fixup!` passes the hook and fails CI. CI checks every pushed commit the same
  way, Dependabot's for attribution only. By hand:
  `sh scripts/check-commit-msg.sh FILE` or `--range A..B`; the check's own tests
  are `scripts/check-commit-msg.test.sh`, a step of the gate.

## Scripts

| Command | What it does |
| --- | --- |
| `./scripts/gate.ps1` | the gate a change passes before commit: rustfmt, clippy `-D warnings`, the Linux unit tests, the commit-message check's tests, and the frontend's install, lint, format check, typecheck, text check, script tests, Vitest suite and build. Stops at the first failure, ends on a summary |
| `./scripts/gate.ps1 -Group frontend` | one group only (`frontend`, `rust`, `windows`) |
| `./scripts/gate.ps1 -Windows` | adds clippy over the Windows build |
| `./scripts/windows-tests.ps1 -Build -Run` | the unit tests cross-built as Windows executables (files named by cargo's JSON messages), copied to a folder of their own and run on this machine |
| `./scripts/linux-check.ps1` | the crate built for Linux and all its unit tests run there: the only place the Unix code (`keeper/`, `procs/unix.rs`, parts of `pty/`) compiles. Checkout mounted read-only; caches in named volumes |
| `./scripts/test-build.ps1` | the test build, `test-build/luna-test.exe` (below); `-Test` is the Windows tests |
| `./scripts/build-windows.ps1` | the release exe, `src-tauri/target/x86_64-pc-windows-msvc/release/luna.exe` (self-contained; needs only the system WebView2) |
| `./scripts/build-windows.ps1 -Installer` | the signed NSIS setup, `…/release/bundle/nsis/Luna_*_x64-setup.exe` |
| `./scripts/screenshots.ps1 -Out test-build/shots` | headless shots of the UI on the demo data |
| `./scripts/live-tests.ps1` | the live suite on real CLIs and throwaway accounts (`scripts/test-accounts.ps1`); never run by CI, see [testing.md](testing.md) |
| `./scripts/release.ps1` | bump the version, or tag a release for the workflow (see [releasing.md](releasing.md)) |

With Node installed locally, `pnpm install` and `pnpm dev` serve the frontend
alone in a browser (Tauri commands are no-ops; `?demo` loads the fixture).
`pnpm tauri dev` needs a Rust toolchain the project does not provide.

The text check (`pnpm check:text`) keeps every tracked file in US English: no
letters outside the Latin script and no British spellings. A line that must keep
one carries a `lang-check: allow` comment.

## The test build

Test only on the test build, never on the Luna in use
([ADR-0006](adr/0006-isolated-test-flavor.md)):

```powershell
./scripts/test-build.ps1 -Run                  # build and start it
./scripts/test-build.ps1 -Run -Background      # window off screen, unfocused, off the taskbar
./scripts/test-build.ps1 -Run -DevCaller       # plus a scripted MCP caller (implies -Background)
./scripts/test-build.ps1 -Test                 # the Rust tests, built for Windows, run here
```

- Its own identifier (`src-tauri/tauri.test.conf.json`), no updater; log,
  attachments, settings and downloaded CLIs live beside its exe in `test-build/`
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
- Before stopping a core or the PC, check `luna_list` and `luna_sessions` for
  work in progress.

## Screenshots

`./scripts/screenshots.ps1` rewrites `docs/screenshots/*.png` in Docker: the
frontend alone on the invented chats and accounts in `src/dev/demo/` (`pnpm dev`
+ `?demo`), photographed by headless Chromium. Shooting a running Luna would
shoot whatever is open in it, and the fixture retakes all three from one state.

## CI

`.github/workflows/ci.yml` runs on every branch push, every pull request and by
hand, with the same scripts:

| Job | Runs |
| --- | --- |
| `frontend` | `gate.ps1 -Group frontend`, then the commit-message check over the pushed range |
| `rust-linux` | `gate.ps1 -Group rust` in `docker/linux-check.Dockerfile`'s image |
| `windows-build` | `gate.ps1 -Group windows` and `windows-tests.ps1 -Build`, uploading the test executables |
| `windows-tests` | runs those executables on a `windows-2025` runner |

`.github/workflows/release.yml` runs only on `v*` tags and by hand
([Releases](#releases)). Actions are pinned by commit; Dependabot (`.github/dependabot.yml`) moves them,
the crates, the npm packages and the images weekly.

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
  (`release-verify.ps1`), builds and signs in the `release` environment
  (`build-windows.ps1`, `release-bundle.ps1`), writes SBOMs (`sbom.ps1`) and
  attestations, and leaves a **draft** release: setup, `.sig`, `latest.json`,
  CycloneDX SBOMs and `SHA256SUMS`. `latest.json`'s `core_fingerprint` comes from
  `core-fingerprint.txt`, which `build.rs` leaves beside the exe, so no runner has
  to run the Windows binary. The logic these scripts share is
  `scripts/release-lib.ps1`, tested in the gate.
- No bare exe is published: the updater installs into Luna's install folder
  ([ADR-0011](adr/0011-private-data-dir-beside-the-exe.md)).
- The updater key is a private minisign key kept outside the repo; its public half
  is in `tauri.conf.json`. `build-windows.ps1 -Installer` takes it from the file
  `-UpdaterKey` or `LUNA_UPDATER_KEY` names, or from `TAURI_SIGNING_PRIVATE_KEY`
  (as the workflow has it); `-Unsigned` builds the setup without its `.sig`; a
  build without `-Installer` needs no key. Authenticode-signing the exe would need
  a certificate, added under `bundle.windows.signCommand`.
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
   `agents/`, respect the [least-privilege contract](adr/0007-least-privilege-for-agent-tools.md),
   update `src-tauri/src/luna_guide.md` and the threat model's matrix.
6. **Check.** `./scripts/gate.ps1`, `./scripts/linux-check.ps1` for Unix code,
   and the test build for anything a person sees. Update the user guide and the
   architecture page the change touches, and record a consequential decision as
   an ADR ([template](adr/0000-template.md)).
