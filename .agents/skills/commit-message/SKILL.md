---
name: commit-message
description: How to write a commit message for this repository in the form its commit-msg hook and CI enforce (scripts/check-commit-msg.sh) — a sentence of at most 72 characters saying what the change does, no type prefix, a blank line, then why — with ten pairs from this history, the body's shape, when a path prefix fits, the merge, revert, bump and fixup forms. Use it whenever you are about to commit, write, reword, amend or squash a commit message, write a revert's or a merge's message, or the hook or CI refused one, even if the ask is only "commit this" or "fix the message".
---

# Commit message

`scripts/check-commit-msg.sh` is the rule; CONTRIBUTING.md ("Commits") says it in
prose. Write the message to a file and run the check before committing:

```sh
sh scripts/check-commit-msg.sh test-build/msg.txt && git commit -F test-build/msg.txt
```

A refusal names the rule, the line and its length, so fix what it names and run
it again. The hook runs the same check (`git config core.hooksPath .githooks`),
and CI runs it over every pushed commit.

## The subject says what the change does

Someone reading `git log --oneline` should learn what Luna, a script or a file
does now, not what kind of change it was: "An elevated run takes back an open
file it made, owned by Administrators" tells the story; "fix: file ownership"
tells nothing. So the subject is a sentence, in at most 72 characters, with no
period at the end, and no Conventional Commits type prefix (`docs:`, `fix:`,
`feat(scope)!:`): the type is in the sentence. It starts with a capital letter,
a digit, or the path-like name of what it is about.

A path prefix fits when the change is about one file or module and that name is
what a reader would search for: a module split or moved, a script's own
behavior. `agents/wait.rs: luna_wait on one session or several`,
`screenshots.ps1 looks for the Windows fonts only on Windows`. A change in
behavior wants the sentence, not the path. An area word with a colon (`ipc:`,
`panes:`) is not a path and is refused as lowercase prose: name the folder
(`ipc/:`) or say the sentence.

A subject that will not fit in 72 characters usually holds two changes, or its
own why. Move the why to the body; split a second change into its own commit.

## The body: why first

Line 2 is blank. The first paragraph says why, as it was seen: the failing test,
the symptom, the reader's need, what was wrong with the old way. Then what
changed and where things moved (a module renamed, a function now elsewhere),
so that a reader of the diff knows what to look for. Wrap at 72; a line with a
URL, an indented line (a command, output) and a single unbreakable token may run
on. An issue is a sentence: "Fixes #12." A small change whose subject says it
all needs no body.

```
An elevated run takes back an open file it made, owned by Administrators

CI's windows-tests failed secret::tests::a_file_planted_where_one_is_
written_in_place_is_moved_aside: the runner runs as the built-in
Administrator, whose token gives new files to BUILTIN\Administrators,
not to the user. [...] a real bug, since an elevated Luna on such a
machine would move aside every log and lock it had itself left open.

found() now takes back a loose file owned by this user or by the
token's default owner (TokenOwner, acl::maker_sid) [...]
```

## Ten pairs from this history

| Refused | Instead |
| --- | --- |
| `docs: the spawn sequence's preparation is schematic, with each CLI's exact order beside it` (type prefix, 90) | `Spawn preparation is schematic, each CLI's exact order beside it` |
| `check-text passes over symbolic links and directories, as a skill's .claude/skills link is` (90) | `check-text passes over symbolic links and directories`; the "as" clause opens the body |
| `screenshots: look for the Windows fonts only on Windows` (lowercase prose) | `screenshots.ps1 looks for the Windows fonts only on Windows` |
| `ipc: the transport in core.ts, each domain's commands and DTOs in a module of its own` (lowercase prose, 85) | `ipc/: the transport in core.ts, each domain's commands and DTOs apart` |
| `panes: what one terminal is made of moves out of create() into terminal/` (lowercase prose) | `panes/terminal/: what one terminal is made of, out of create()` |
| `accountChoice always says why it took the one it took, and an askId a helper never had is called unknown` (lowercase prose, 104) | `An accountChoice always says why; an askId a helper never had is unknown` |
| `Tests: settings defaults that are not stock, a late answer that must lose, and no state leaking between bridge tests` (116) | `Tests of non-stock defaults, a losing late answer, bridge tests apart` |
| `Review fixes: the library pass holds no helper up and freezes no window, a window the core let go goes back to it, a held helper keeps nothing awake` (148) | three commits: `The library pass holds no helper up and freezes no window`, ... |
| `A chat's leftovers are measured as a helper's, the power log says hourly what keeps the PC up, a turn that comes to nothing spares what earlier ones left, ...` (258) | one commit per sentence: `A chat's leftovers are measured as a helper's` |
| `Luna 0.8.3: a turn gone quiet is told of, agents turn the PC off the way Luna does, and the guide says how to work with helpers` (127) | `Luna 0.8.3`; the list is the CHANGELOG's |

## The forms git writes itself

- **Merge**: git's or GitHub's own `Merge branch 'x' into y`, `Merge pull
  request #N from ...`. Not held to the form; its body may say what conflicted.
- **Revert**: `git revert` writes `Revert "<the subject>"` and `This reverts
  commit <sha>.`; keep both and add a paragraph on why it goes back.
- **Bump**: exactly `Luna X.Y.Z`, written by `scripts/release.ps1 -Bump`.
- **fixup! / squash!**: `git commit --fixup <sha>` passes the hook so that a
  review's follow-ups can be made, but CI refuses them: squash before merging
  (`GIT_SEQUENCE_EDITOR=true git rebase --autosquash <base>`, non-interactive).
- **Dependabot**: its commits are known by their author and left to their own
  form. Nothing to rewrite.

Whatever the form, no attribution: no `Co-Authored-By:`, no robot emoji, no
"Generated with ...". Authorship is git's author field.
