# ADR-0035: Squash a slice into one commit; merge waves and releases

- **Status:** Accepted
- **Date:** 2026-11-26

## Context

Almost every change to Luna is written by coding agents working in parallel
worktrees. In the 30 days to 2026-11-26 master took 1,800 commits, 318 of them
on its first-parent line, and 117 pull request merges whose subjects were
GitHub's `Merge pull request #N from …/slice/<name>`. Every commit was held to
the sentence rule (CONTRIBUTING.md, "Commits") by the commit-msg hook and CI,
so each one explained itself, but the history GitHub shows was hundreds of
small steps a day, and `git log --first-parent` read as branch names. Every
refused message also cost an agent another round.

On the table: merge commits only (every step kept, the noise read past with
`--first-parent`), squash only (one commit per pull request, waves of slices
collapsed into one), rebase merging (steps kept, rewritten onto master), and
squashing slices while merging waves and releases. GitHub's merge queue is
not offered for a repository on a personal account.

## Decision

A `slice/*` pull request, and any other from a branch that is not
`iteration/*` or `release/*` (a contributor's fork included), is squashed: one
commit on master, its subject the pull request's title with GitHub's ` (#N)`,
its body the description. Inside a wave each slice is squashed into the
`iteration/*` branch the same way, and that branch, like a `release/*` one,
is merged into master with a merge commit titled and described from its pull
request, so each slice keeps its one commit. Rebase merging is off.

The sentence rule moves with the history: `scripts/check-commit-msg.sh` holds
commits on `master`, `iteration/*` and `release/*` to it, and only refuses AI
attribution elsewhere; `.github/workflows/pr.yml` (`pr-message`) holds a pull
request's title and description to it, since they become the commit.
`docs/development.md` ("Repository settings") records the GitHub settings that
carry this out.

## Consequences

- Master reads as one sentence per change, and a change reverts and bisects
  as one commit. Agents' commits on a slice need no polishing.
- `git blame` lands on a slice's commit, not on the step inside it; the
  slice's description has to carry the why its steps used to.
- A squashed slice's branch is not an ancestor of master, so a branch that
  builds on it merges master in after the squash rather than expecting git
  to see the slice as merged.
- A pull request whose history holds a finding `.gitleaksignore` lists is
  merged with a merge commit, or the entries name commits master never holds.
- The merge method is the merger's choice on GitHub; nothing refuses a
  squashed `iteration/*` pull request, so the rule lives in AGENTS.md and
  CONTRIBUTING.md.
