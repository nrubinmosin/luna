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

The PowerShell 7.4+ scripts build Rust in Docker with the pinned toolchain.
Script checks and Windows test executables run on the host.
See [docs/development.md](docs/development.md).

```powershell
git config core.hooksPath .githooks     # once per clone
./scripts/gate.ps1                      # script checks, Rust formatting, lints and Linux tests
./scripts/gate.ps1 -Windows             # plus clippy over the Windows build
./scripts/linux-check.ps1               # Unix code and its tests
./scripts/test-build.ps1 -Run -Background   # try it on the isolated test build
```

Test only on the test build, and only on throwaway accounts under
`test-build/accounts`.

## Commits

Master's history is the record: each change on it is told as a sentence, what
the behavior is now, not what kind of change it was. A pull request from an
ordinary branch (`slice/*`, a fork's) is squashed, so its commits on the way
are yours to shape and only need no attribution; its title and description
become the commit and follow the rules below ([Pull requests](#pull-requests)).
Commits on `master`, `iteration/*` and `release/*` reach master as they are
and follow them each. The commit-msg hook and CI refuse what strays, by the
branch the commit is on; `sh scripts/check-commit-msg.sh FILE` checks a draft
and `sh scripts/check-commit-msg.sh --range origin/master..HEAD` a branch.

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
  `Revert "..."` and the release bump `Luna X.Y.Z`. On a branch whose
  commits are kept, a `fixup!`, `squash!` or `amend!` commit passes the hook
  and fails CI: squash it before the merge.
  A message the hook refused was never committed: fix it and commit again
  (`git commit -e -F <file>`), not `--amend`.
- Everything in US English (`pwsh -File scripts/check-text.ps1`).
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

A pull request is squash-merged: its title becomes the commit's subject, with
GitHub's ` (#N)` after it, and its description the commit's body. So the title
is a commit subject (at most 72 characters with that suffix, about 65 without)
and the description is the body: why first, wrapped at 72, no Markdown
headings or checklists. The `pr-message` check holds both to the rules above,
and runs again when either is edited. An `iteration/*` or `release/*` pull
request is merged with a merge commit instead, titled and described the same
way.

Record notable changes in a [changelog fragment](changes/README.md). Product
changes that need no release note still need a fragment with `### None` and
a line explaining why.

CI runs the PowerShell checks, the commit-message check, the Linux Rust gate,
the Windows build and the Windows unit tests. Before you open it, check:

- `./scripts/gate.ps1` passes (`-Windows` for Windows-only code, and
  `./scripts/linux-check.ps1` for Unix code), and the change was tried on the
  test build, on throwaway accounts.
- What each CLI does here is in the description ("Codex needs nothing,
  because …" is fine).
- The docs moved with the code: the user guide or an architecture page, an
  ADR for a decision costly to reverse, `docs/cli-compatibility.md` for a new
  CLI observation.
- US English (`pwsh -File scripts/check-text.ps1`), nothing obsolete or
  scratch left in the tree.

## Issues

Every issue starts from a form: **Bug** and **Feature** for anyone, **Task** for
work the maintainer or an agent takes on, with a goal, the evidence and a
"Done when" list someone else can check. Bug, Feature and Task are issue types;
questions go to [Discussions](https://github.com/nrubinmosin/luna/discussions)
and vulnerabilities to [SECURITY.md](SECURITY.md). Search open and closed
issues first: a comment on an existing one beats a new one, and a larger
effort is a parent issue with sub-issues.

The labels, kept by `scripts/labels.ps1` (run it after changing this table):

| Label | When |
| --- | --- |
| `area:window` | the native window: panes, boards, chats, settings |
| `area:sessions` | launching the CLIs, terminals, status, worktrees, the library |
| `area:agents` | helpers and leads, MCP tools, notices |
| `area:accounts` | accounts, usage limits, the managed CLI copies |
| `area:core` | the core process, power, updates, the installer |
| `area:repo` | CI, scripts, docs and skills for contributors |
| `cli:claude`, `cli:codex` | only that CLI has the problem or needs the change |
| `needs-info` | waiting on the reporter; set by the maintainer with a comment saying what is missing |
| `blocked` | waiting on another issue or an outside change, named in a comment |
| `good first issue` | small and self-contained, a fit for a first contribution |
| `help wanted` | the maintainer would welcome a pull request |

The maintainer sets them in triage; an issue carries the areas it touches and
no `cli:` label when both CLIs are concerned. A question that blocks the work
is a comment on the issue, answered by a comment opening with `Decision:`.
GitHub's close reasons (completed, not planned, duplicate) replace the old
`duplicate`, `invalid` and `wontfix` labels.

By contributing you agree that your contribution is dual licensed under
MIT OR Apache-2.0, as described in the [README](README.md#license).
