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

One logical change per commit, told as a sentence: what the behavior is now,
not what kind of change it was. The commit-msg hook and CI refuse what strays;
`sh scripts/check-commit-msg.sh FILE` checks a draft and
`sh scripts/check-commit-msg.sh --range origin/master..HEAD` a branch.

- **The subject says what the change does**, in at most 72 characters, with no
  period at the end. It starts with a capital letter, a digit or the path-like
  name of what it is about (`agents/wait.rs: luna_wait on one session or
  several`). No type prefix (`docs:`, `fix(scope)!:`): the type is in the
  sentence.
- **A blank line, then why.** The body says why first, then what changed and
  where it moved, wrapped at 72. A line with a URL, an indented line (a command,
  a table row) or a single unbreakable token may run on. An issue is a
  sentence: "Fixes #12."
- **No attribution trailers**: no `Co-Authored-By:`, no robot emoji, no
  "Generated with ...". Authorship is git's author field only.
- **The forms git writes itself stay as they are**: `Merge ...`,
  `Revert "..."` and the release bump `Luna X.Y.Z`. A `fixup!`, `squash!` or
  `amend!` commit passes the hook and fails CI: squash it before the merge.
  A message the hook refused was never committed: fix it and commit again
  (`git commit -e -F <file>`), not `--amend`.
- Everything in US English (`pnpm check:text`).
- Match the surrounding code: naming, idioms, comment density; comments say why.
- Delete what a change makes obsolete in the same change; commit no scratch
  files.

From this repository's history:

| | Subject |
| --- | --- |
| good | `An elevated run takes back an open file it made, owned by Administrators` |
| good | `agents/wait.rs: luna_wait on one session or several` |
| good | `procs.rs becomes a folder, each OS's part a file of its own` |
| refused | `docs: the spawn sequence's preparation is schematic, with each CLI's exact order beside it`: a type prefix, and 90 characters. Instead: `Spawn preparation is schematic, each CLI's exact order beside it` |
| refused | `check-text passes over symbolic links and directories, as a skill's .claude/skills link is`: 90 characters. The "as" clause opens the body: `check-text passes over symbolic links and directories` |
| refused | `fix: gate step for the commit-message tests.`: a type prefix, lowercase prose and a period. Instead: `The gate runs the commit-message check's tests` |

## Pull requests

A pull request must pass CI: the frontend gate (including the commit-message
check), the Linux Rust gate, the Windows build and the Windows unit tests. Fill in
the template: what changed, what each CLI does, how you tested, and which docs
you updated (user guide, architecture page, ADR, CLI compatibility notes).

By contributing you agree that your contribution is dual licensed under
MIT OR Apache-2.0, as described in the [README](README.md#license).
