# ADR-0004: Everything builds in Docker on pinned toolchains

- **Status:** Accepted
- **Date:** 2026-08-12 (the first commit shipped `build-windows.ps1` and
  `docker/windows-build.Dockerfile`); the single gate and pins followed on
  2026-11-07 (`379ce81`, `5f25b02`, `dfe1186`)

## Context

The maintainer's machine has no local Rust or Node toolchain, and the release
target (`x86_64-pc-windows-msvc`) needs the Windows SDK and NSIS. Local
toolchains drift; CI on a different setup produces failures that do not
reproduce at home.

## Decision

Every build, lint and test runs in Docker images defined in `docker/`, driven by
PowerShell 7.4+ scripts in `scripts/` that run the same on any host:

- Rust is pinned in `src-tauri/rust-toolchain.toml` and the images'
  `FROM rust:<version>`; Node in `.node-version` and `FROM node:<version>`;
  `scripts/gate.ps1` fails when they disagree. Every `FROM` names its image by
  tag and digest.
- The Windows build cross-compiles with `cargo-xwin`; the Unix code is built
  and tested by `scripts/linux-check.ps1`; Windows unit tests are cross-built
  and run on a Windows host (`scripts/windows-tests.ps1`).
- Images are tagged by a hash of what they are built from (`scripts/lib.ps1`),
  so checkouts with different Dockerfiles never share one.
- CI (`.github/workflows/ci.yml`) runs the same scripts.

## Consequences

- A CI failure reproduces with the same command locally.
- First builds are slow; named volumes cache the cargo registry, the Windows
  SDK and `node_modules`.
- `pnpm tauri dev` needs a Rust toolchain the project does not provide; the
  supported loop is the test build ([development](../development.md)).
- Dependabot moves the images, crates, packages and pinned Actions weekly.
