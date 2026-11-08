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
| Rust nightly, cargo-fuzz (fuzzing only) | `docker/fuzz.Dockerfile` and `src-tauri/fuzz/rust-toolchain.toml`; `scripts/fuzz.ps1` fails when the two disagree |

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
  `fixup!`, `squash!` or `amend!` passes the hook and fails CI. CI checks every
  pushed commit the same way, Dependabot's for attribution only. By hand:
  `sh scripts/check-commit-msg.sh FILE` or `--range A..B`; the check's own tests
  are `scripts/check-commit-msg.test.sh`, a step of the gate.

## Scripts

| Command | What it does |
| --- | --- |
| `./scripts/gate.ps1` | the gate a change passes before commit: rustfmt, clippy `-D warnings`, the Linux unit tests, the commit-message check's tests, and the frontend's install, lint, format check, typecheck, text check, Tauri version check, script tests, Vitest suite and build. Stops at the first failure, ends on a summary |
| `./scripts/gate.ps1 -Group frontend` | one group only (`frontend`, `rust`, `windows`) |
| `./scripts/gate.ps1 -Windows` | adds clippy over the Windows build |
| `./scripts/windows-tests.ps1 -Build -Run` | the unit tests cross-built as Windows executables (files named by cargo's JSON messages), copied to a folder of their own and run on this machine |
| `./scripts/linux-check.ps1` | the crate built for Linux and all its unit tests run there: the only place the Unix code (`keeper/`, `procs/unix.rs`, parts of `pty/`) compiles. Checkout mounted read-only; caches in named volumes |
| `./scripts/test-build.ps1` | the test build, `test-build/luna-test.exe` (below); `-Test` is the Windows tests |
| `./scripts/build-windows.ps1` | the release exe, `src-tauri/target/x86_64-pc-windows-msvc/release/luna.exe` (self-contained; needs only the system WebView2) |
| `./scripts/build-windows.ps1 -Installer` | the signed NSIS setup, `…/release/bundle/nsis/Luna_*_x64-setup.exe` |
| `./scripts/screenshots.ps1 -Out test-build/shots` | headless shots of the UI on the demo data |
| `./scripts/fuzz.ps1` | the fuzz targets over the transcript readers, built and run in `docker/fuzz.Dockerfile`'s image; `-Seconds`, `-Reproduce <file>`, `-Minimize` (see [testing.md](testing.md#fuzzing)) |
| `./scripts/live-tests.ps1` | the live suite on real CLIs and throwaway accounts (`scripts/test-accounts.ps1`); never run by CI, see [testing.md](testing.md) |
| `./scripts/release.ps1` | bump the version, or tag a release for the workflow (see [releasing.md](releasing.md)) |
| `./scripts/supply-chain.ps1` | the supply-chain checks: cargo-deny, `pnpm audit` and gitleaks ([below](#supply-chain)) |
| `./scripts/third-party-notices.ps1 -Out <file>` | the third-party notices the installer carries (`build-windows.ps1 -Installer` makes them first) and the release publishes: each crate the exe links and each npm package the interface bundles, with its license texts and source; the Rust standard library with the inventory its toolchain carries; and, from `third-party-licenses/components.json`, the helpers the build tools write into the bundle and what the installer carries beside (NSIS, its Tauri plug-in, the WebView2 loader, the VC runtime). The WebView2 runtime is downloaded by the installer when missing, not shipped. Texts a package does not publish are kept in `third-party-licenses/` (its README); the script fails on a shipped package without one, and the gate on a components entry that the lockfiles or the bundle have moved past |

With Node installed locally, `pnpm install` and `pnpm dev` serve the frontend
alone in a browser (Tauri commands are no-ops; `?demo` loads the fixture).
`pnpm tauri dev` needs a Rust toolchain the project does not provide.

The text check (`pnpm check:text`) keeps every tracked file in US English: no
letters outside the Latin script and no British spellings. A line that must keep
one carries a `lang-check: allow` comment.

The Tauri version check (`pnpm check:tauri`, `scripts/check-tauri-versions.mjs`)
holds each Tauri crate in `src-tauri/Cargo.lock` and its npm package in
`pnpm-lock.yaml` (`tauri` and `@tauri-apps/api`, `tauri-plugin-<name>` and
`@tauri-apps/plugin-<name>`) to one major.minor, the rule `tauri build` refuses
a build by. The gate never runs `tauri build`, and Dependabot moves the cargo and
npm sides in separate PRs, so a bump to one side fails here first: move the other
side to match in the same PR.

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
| `windows-build` | `gate.ps1 -Group windows` and `windows-tests.ps1 -Build`, uploading the test executables; then `third-party-notices.ps1`, as the release makes it on a Linux runner |
| `windows-tests` | runs those executables on a `windows-2025` runner |

`.github/workflows/fuzz.yml` runs each fuzz target for five minutes on Mondays
and by hand, keeping only what one finds as an artifact (see
[testing.md](testing.md#fuzzing)).

`.github/workflows/security.yml` runs the supply-chain checks and CodeQL on
every pull request, every push to master, weekly and by hand
([Supply chain](#supply-chain)).

`.github/workflows/release.yml` runs only on `v*` tags and by hand
([Releases](#releases)). Actions are pinned by commit; Dependabot (`.github/dependabot.yml`) moves them,
the crates, the npm packages and the images monthly, one grouped PR per ecosystem; a new
Node or Rust major is left to a hand-made migration (`.github/dependabot.yml` says why).

## Supply chain

`./scripts/supply-chain.ps1` runs, each in Docker on a pinned tool:

| Check | What fails it |
| --- | --- |
| `tests` | the tests of the checks themselves (`scripts/supply-chain-lib.test.ps1`): the ignore rules, `deny.toml` read as TOML, gitleaks' output kept free of secrets |
| `deny` | cargo-deny 0.20.2 (release binary, SHA-256 pinned in the script) with `src-tauri/deny.toml`: a RustSec advisory anywhere in the tree (vulnerable, unmaintained, unsound), a license outside the allowlist, a wildcard requirement, a crate from anywhere but crates.io. Duplicate versions and yanked crates are warnings |
| `audit` | `pnpm audit`, production and dev packages: a high or critical advisory. Moderate and low ones are listed |
| `secrets` | gitleaks 8.30.1 (image by digest) over the history reachable from `HEAD`, or `-Range <base>..<head>`. A finding prints its rule, file, line and commit only, never the text it matched: gitleaks' own redaction covers each finding's secret, not the rest of its line |

```powershell
./scripts/supply-chain.ps1                     # all four; exit 1 if any fails
./scripts/supply-chain.ps1 -Check deny,audit   # some of them
```

They need the network (the advisory databases, the registries), and their answer
changes with no change to the repo, so the gate does not run them and stays
hermetic. In CI, `security.yml` runs each check as a job of its own (`gitleaks`
over a pull request's commits, or master's whole history on schedule and by
hand) and CodeQL over the workflows, the TypeScript and the Rust, build-mode
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
- an npm package: an entry in `pnpm-audit-ignores.json`'s `ignores`:
  `{ "id": "GHSA-...", "package": "...", "reason": "...", "expires": "YYYY-MM-DD" }`.

A reason of a few words, or only whitespace, is refused. An ignore goes in through a pull request like any change, with the advisory and
why it does not reach Luna in its description; three months is the usual
expiry. The licenses `deny.toml` allows are the ones the tree needs, each
unusual one justified there; a new license in the tree is decided there, not
allowed in bulk.

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
  (`release-sign.ps1`), writes SBOMs (`sbom.ps1`) and attestations, and leaves a
  **draft** release: setup, `.sig`, `latest.json`,
  CycloneDX SBOMs and `SHA256SUMS`. `latest.json`'s `core_fingerprint` comes from
  `core-fingerprint.txt`, which `build.rs` leaves beside the exe, so no runner has
  to run the Windows binary. The logic these scripts share is
  `scripts/release-lib.ps1`, tested in the gate.
- No bare exe is published: the updater installs into Luna's install folder
  ([ADR-0011](adr/0011-private-data-dir-beside-the-exe.md)).
- The updater key is a private minisign key kept outside the repo; its public half
  is in `tauri.conf.json`. No build takes it: `build-windows.ps1 -Installer` makes
  the setup unsigned, since `tauri build` runs every dependency's build code.
  `release-sign.ps1` signs it afterwards with the key from the file `-UpdaterKey`
  or `LUNA_UPDATER_KEY` names, or from `TAURI_SIGNING_PRIVATE_KEY` (as the
  workflow has it), running only the Tauri CLI's `signer sign`, held to the
  integrity `pnpm-lock.yaml` pins, in the pinned Node image without network; it
  checks the signature against the public key (`updater-signature.mjs`) and puts
  it into `latest.json`. Authenticode-signing the exe would need a certificate,
  and the same separation: a `bundle.windows.signCommand` runs inside the build.
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
