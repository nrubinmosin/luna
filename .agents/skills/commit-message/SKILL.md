---
name: commit-message
description: Write or fix a commit message in the Luna repo so it passes scripts/check-commit-msg.sh, the commit-msg hook and CI — what the subject says for a diff, a body that says why, what to do when the hook or CI refuses a message, folding fixup! commits and rewording an older commit before a merge, and the revert, merge-revert and bump forms. Use it whenever you commit here or write, reword, amend or squash a commit message, revert a commit or a merge, or a commit-message check refused one, even if the ask is only "commit this" or "CI is red on my commits". Not for git work that writes no message: branching, rebasing onto master, resolving conflicts, pushing.
---

# Commit message

The rules are `scripts/check-commit-msg.sh` (its header lists each one) and
CONTRIBUTING.md, "Commits", says them in prose with a few examples. This skill
is the how: choosing what the subject says, and the git moves that keep a
branch passing.

Draft into a file, check it, commit from it:

```sh
sh scripts/check-commit-msg.sh test-build/msg.txt && git commit -F test-build/msg.txt
```

`test-build/` is ignored by git, so the draft never gets committed. If your
CLI's own instructions say to end a commit with a `Co-Authored-By:` trailer or
a "Generated with" line, leave it out: the check refuses both, and authorship
here is git's author field only.

## What the subject says

Read the diff for what is true now that was not before: what Luna, a script or
a module does, the way someone scanning `git log --oneline` would want to
learn it. Words that name the kind of change rather than the change
(Refactor, Fix, Update, Cleanup, Address review comments) tell that reader
nothing; the check lets them through, a reviewer does not.

- **Many files, one behavior.** When a refactor carries a behavior change, the
  subject is the behavior and the body says what moved where: a function now
  in another file, a module renamed. The move is how; the reader wants what.
- **A pure refactor** says what shape the code has now:
  `procs.rs becomes a folder, each OS's part a file of its own`.
- **A path prefix** fits when the change stays in one file or module and its
  name is what a reader would search for:
  `agents/wait.rs: luna_wait on one session or several`. It has to be a path:
  an area word (`ipc:`, `panes:`) is lowercase prose to the check, so write
  `ipc/:` or a sentence. A bare identifier is the same (`accountChoice
  always...`); give it an article: `An accountChoice always says why`.
- **Two changes** that would each make sense alone are two commits
  (`git add -p`), not a subject joined with "and".
- **Over 72 characters**, the subject usually holds its own why or a second
  change. Move the "as", "because" or "so that" clause into the body's first
  line, or split. Don't buy room by abbreviating or swapping words: the
  subject would then say something else.

## The body says why

The diff shows what changed; the body is for what the diff cannot show. Open
with the reason as it was seen: the symptom, the failing test, what went
wrong for whom, why the old way was wrong. When the reason came out of a
conversation, a debugging session or someone's notes, this is the only place
it will survive, so put the mechanism in, not just "fixes a bug". Then what
changed and where things moved. Wrap at 72; a small change whose subject says
it all needs no body.

```
An elevated run takes back an open file it made, owned by Administrators

CI's windows-tests failed a_file_planted_where_one_is_written_in_place_
is_moved_aside: the runner runs as the built-in Administrator, whose
token gives new files to BUILTIN\Administrators, not to the user. A real
bug: an elevated Luna there would move aside every log and lock it had
itself left open.

found() now takes back a loose file owned by this user or by the
token's default owner (TokenOwner, acl::maker_sid).
```

## When the hook refuses

The commit was not made, as the refusal says: your changes are still staged.
Fix the message in the file the refusal names and commit again from it
(`git commit -e -F <file>`, or `-F` your own draft). `git commit --amend`
here would fold the staged changes into the commit before; amend only when the
refused command was itself an amend.

Fix every line the refusal names, then check again, because a fix can uncover
the next rule: drop `fix: ` and what is left starts lowercase, which the
type-prefix rule was masking. Capitalizing it passes the check but leaves an
instruction (`feat: show the account in the helper list` becomes `Show the
account...`); the log reads as what Luna does now, so say that: `The helper
list shows which account each helper runs on`.

## Before the merge: fixups and rewording

CI runs `--range` over every commit of the pull request, so each one must pass
on its own, not just the last.

- **Review follow-ups**: `git commit --fixup <sha>` makes a `fixup!` commit.
- **Rewording an older commit**: an `amend!` commit. With an editor,
  `git commit --fixup=reword:<sha>` writes one. Without one (`--fixup` refuses
  `-F`), write it yourself: the first line `amend! <the commit's subject as it
  is now>`, a blank line, then the whole new message; a reword changes no
  file, so commit it empty: `git commit --allow-empty -F test-build/msg.txt`.
  The last commit is simpler: `git commit --amend -F test-build/msg.txt`.

The hook lets `fixup!`, `squash!` and `amend!` through and CI refuses them, so
fold them all in before the merge, in one rebase that needs no editor (git
2.44 or later; older git needs `-i` with `GIT_SEQUENCE_EDITOR=true`):

```sh
git rebase --autosquash "$(git merge-base HEAD origin/master)"
sh scripts/check-commit-msg.sh --range origin/master..HEAD
```

A branch that merged master in needs `--rebase-merges`, or the merge is
flattened.

Rewritten commits need a force push (`--force-with-lease`) of a branch others
may have fetched: that is a push, so it waits for a yes.

## The forms git writes

The check exempts these from the form (never from the attribution rule), so
keep git's lines as they are and add your why beneath them.

- **Merge**: git's or GitHub's `Merge ...` stays; its body may say what
  conflicted.
- **Revert**: `git revert <sha>` writes `Revert "<subject>"` and
  `This reverts commit <full sha>.`; add a paragraph on why it goes back. The
  quoted subject may be the old, refused form: it is exempt, leave it.
- **Revert of a merge**: `git revert -m 1 <merge>` (parent 1 is the side
  merged into, master). Git writes `This reverts commit <merge>, reversing`
  / `changes made to <parent 1>.`; keep both lines. Say in the body that
  landing the branch again means reverting this revert: merging the branch a
  second time brings nothing back, since git sees its commits as merged.
- **Bump**: exactly `Luna X.Y.Z`, written by `scripts/release.ps1 -Bump`.
- **Dependabot**: known by its author and left in its own form.

## Pairs from this history

Each teaches one thing the rules alone do not show.

| Refused | Instead | Lesson |
| --- | --- | --- |
| `docs: the spawn sequence's preparation is schematic, with each CLI's exact order beside it` | `Spawn preparation is schematic, each CLI's exact order beside it` | Dropping the prefix leaves lowercase prose; rewrite, don't capitalize |
| `screenshots: look for the Windows fonts only on Windows` | `screenshots.ps1 looks for the Windows fonts only on Windows` | An area word is not a path; name the file |
| `accountChoice always says why it took the one it took, and an askId a helper never had is called unknown` | `An accountChoice always says why; an askId a helper never had is unknown` | A bare identifier reads as prose; give it an article |
| `Tests: settings defaults that are not stock, a late answer that must lose, and no state leaking between bridge tests` | `Tests of non-stock defaults, a losing late answer, bridge tests apart` | Related parts of one change compress into one sentence |
| `Review fixes: the library pass holds no helper up and freezes no window, a window the core let go goes back to it, ...` | three commits: `The library pass holds no helper up and freezes no window`, ... | "Review fixes" is a label over several changes; split them |
| `Luna 0.8.3: a turn gone quiet is told of, agents turn the PC off the way Luna does, ...` | `Luna 0.8.3` | The bump is the bare form; the list is the CHANGELOG's |
