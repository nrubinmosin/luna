# ADR-0029: Move every pin in the project's own commits; Dependabot only alerts

- **Status:** Accepted
- **Date:** 2026-11-18

## Context

What Luna is built from is pinned: the crates in `src-tauri/Cargo.lock`, the
npm packages in `pnpm-lock.yaml`, every Action in `.github/workflows/` by
commit SHA, every base image in `docker/` by tag and digest, and the
toolchains in `src-tauri/rust-toolchain.toml`, `rust-version` in
`src-tauri/Cargo.toml` and `.node-version`. The build runs all of it, the
release build with the updater's signing key present
([threat model](../security/threat-model.md), B6), so whoever moves a pin
chooses code that runs there.

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
3. Alerts only: Dependabot reports advisories against what is pinned, and
   every change to a pin is a commit of the project's own.

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
- Advisories are seen in two places: Dependabot alerts in the repository's
  Security tab (crates, npm packages and Actions; not the Docker images), and
  cargo-deny and `pnpm audit` in `.github/workflows/security.yml` on every
  pull request, every push to master and weekly (`scripts/supply-chain.ps1`
  locally), where a high or critical advisory fails the check. One is acted on
  by a commit that moves the package to a fixed version or, where there is
  none yet or the package cannot move, by an ignore with a reason and an
  expiry ([development](../development.md#supply-chain)).
- Nothing opens a fix on its own: between an advisory and its fix there is a
  person's turn. A Docker base image's own distro packages get no alert at
  all; they move when the image's digest is bumped by hand.
- Dependabot's earlier commits stay in master's history in their own form. CI
  checks the commits a push or a pull request brings, never those master
  already holds, so they are not checked again.
