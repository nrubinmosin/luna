# Contributing to Luna

Thanks for your interest. Luna is maintained by one person, so small, focused
changes with a clear reason are the easiest to take.

## Before you start

- Read [AGENTS.md](AGENTS.md): the rules for anyone, person or agent, changing
  Luna (plan first, where things go, both providers, US English).
- For anything beyond a small fix, open an issue first and describe what you
  would change for **each** CLI (Claude Code and Codex).
- Security problems go through the private channel in [SECURITY.md](SECURITY.md),
  never a public issue.

## Build and test

Everything runs in Docker through PowerShell 7.4+ scripts; no local Rust or Node
toolchain is needed. Details: [docs/development.md](docs/development.md).

```powershell
git config core.hooksPath .githooks     # once per clone
./scripts/gate.ps1                      # what CI runs: format, lint, types, tests, build
./scripts/gate.ps1 -Windows             # plus clippy over the Windows build
./scripts/linux-check.ps1               # Unix code and its tests
./scripts/test-build.ps1 -Run -Background   # try it on the isolated test build
```

Test only on the test build, and only on throwaway accounts under
`test-build/accounts`.

## Commits

- One logical change per commit, with a subject that says what the change does.
- Everything in US English (`pnpm check:text`).
- Match the surrounding code: naming, idioms, comment density; comments say why.
- **No attribution trailers**: no `Co-Authored-By:`, no robot emoji, no
  "Generated with ...". Authorship is git's author field only. The commit-msg
  hook and CI enforce this.
- Delete what a change makes obsolete in the same change; commit no scratch
  files.

## Pull requests

A pull request must pass CI: the frontend gate (including the commit-message
check), the Linux Rust gate, the Windows build and the Windows unit tests. Fill in
the template: what changed, what each CLI does, how you tested, and which docs
you updated (user guide, architecture page, ADR, CLI compatibility notes).

By contributing you agree that your contribution is dual licensed under
MIT OR Apache-2.0, as described in the [README](README.md#license).
