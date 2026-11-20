# ADR-0029: Move every pin in the project's own commits; Dependabot only alerts

- **Status:** Superseded in part by [ADR-0030](0030-scope-of-0-20.md)
- **Date:** 2026-11-18

> **Note (0.20).** The npm and Node pins and npm audit went away. The remaining pins still move by hand.

## Context

What Luna is built from is pinned: the crates in `src-tauri/Cargo.lock`, the
npm packages in `pnpm-lock.yaml`, every Action in `.github/workflows/` by
commit SHA, every base image in `docker/` by tag and digest, and the
toolchains in `src-tauri/rust-toolchain.toml`, `rust-version` in
`src-tauri/Cargo.toml` and `.node-version`. The build runs all of it, and
what it builds reaches every installed copy through the updater
([threat model](../security/threat-model.md), B6), so whoever moves a pin
chooses code that runs there and in the app.

Until now Dependabot moved most of them. `.github/dependabot.yml` opened one
grouped pull request a month per ecosystem (cargo, npm, Actions, Docker), and
Dependabot security updates opened one at once for an advisory. Those pull
requests were authored by a bot account, in a message form of its own that
`scripts/check-commit-msg.sh` exempted by author. And a grouped bump seldom
stood alone: an image bump failed the pin check (`Assert-ToolchainPins` in
`scripts/lib.ps1`) until `.node-version` or `rust-toolchain.toml` moved with
it, a Tauri crate failed `scripts/check-tauri-versions.mjs` until its npm half
moved, and a migration (ureq 3, sha2 0.11) was held off by ignores in the
configuration. Someone finished each of them by hand.

On the table:

1. Keep version updates and security updates.
2. Keep security updates only: a pull request per advisory, nothing on a
   schedule.
3. Alerts only: Dependabot reports advisories against what GitHub's
   dependency graph can read of the pins, and every change to a pin is a
   commit of the project's own.

## Decision

Option 3. Dependabot alerts stay on and Dependabot security updates are off
(both repository settings on GitHub), and `.github/dependabot.yml` is gone, so
no version updates are opened either. The lockfiles, the pinned Actions, the
image tags and digests and the toolchain pins move only in the project's own
commits, through the gate and the review every change gets, and under the
commit rules with no exemption for a bot (`scripts/check-commit-msg.sh`).
Dependabot authors no code.

## Consequences

- Only the project's own authors write what lands; a pin moves in a commit
  that says why, together with what it needs (the other half of a Tauri pair,
  a toolchain pin, the code of a migration).
- Nothing moves on a schedule: a pin stays where it was put until someone
  moves it. A refresh is a change of its own, made by hand in the build
  containers (`cargo update`, `pnpm update`, new digests in `docker/`), for
  instance as a release nears or when a tool needs a newer version. Falling
  behind is easy, and catching up costs more the longer it waits.
- What watches each pin for advisories:
  - the crates: cargo-deny in `.github/workflows/security.yml`, and
    Dependabot alerts, from `src-tauri/Cargo.lock`;
  - the npm packages: `pnpm audit` in the same workflow. pnpm 12 writes
    `pnpm-lock.yaml` as two YAML documents, from which GitHub's dependency
    graph detects no dependencies. GitHub's docs list `package.json` as an
    input too, and it pins the direct dependencies exactly, so Dependabot
    alerts may still come for those; the transitive ones are watched by
    `pnpm audit` alone;
  - the Actions, the Docker base images and the toolchains and tools in them
    (Node, Rust, pnpm through `packageManager`, cargo-xwin, the cargo-deny
    binary, the gitleaks image): nothing. GitHub raises alerts for an Action
    pinned by a version, not by a commit SHA as every one here is, and none
    for an image. These move only when the project moves them by hand, and
    reading their releases for fixes is part of moving them.

  The workflow runs on every pull request, every push to master and weekly
  (`scripts/supply-chain.ps1` locally), and a high or critical advisory fails
  it. One is acted on by a commit that moves the package to a fixed version
  or, where there is none yet or the package cannot move, by an ignore with a
  reason and an expiry ([development](../development.md#supply-chain)). A
  check over the unwatched pins (an advisory lookup for the Actions, a
  scanner for the images) could be added later; none runs now.
- Nothing opens a fix on its own: between an advisory and its fix there is a
  person's turn.
- Dependabot's earlier commits in master's history keep their own form until
  a later rewrite of that history rewords them. Until then CI does not check
  them again: it checks the commits a push or a pull request brings, never
  those master already holds.
