---
name: session-handoff
description: How a chat with Luna's Auto-handoff on writes its handoff, the one file the next conversation of the same chat starts from once Luna clears this one. Luna serves this text through luna_session_handoff when the chat's context passes its threshold; no CLI lists it.
---

# Writing the handoff

The next conversation of this chat begins with nothing of this one but your
file and the facts Luna adds. It reads the whole file before it acts. What the
file leaves out is lost in practice: the old transcript stays on disk, but
reading it back costs the context the handoff is meant to save. Write for a
capable stranger: every line should help it take the right step or keep it
from a wrong one.

If context is short, write the next action, the user's words and the state
first. A short file with exact quotes beats a long one in your paraphrase.

## The file

Markdown, with these headings in this order. Under one with nothing to say,
write "None." so the reader knows you looked.

1. `## Goal`: what the work is for and what done looks like, in two or three
   lines; name the plan or spec to read first, if any.
2. `## Next action`: the one step to take first, concrete enough to start
   without asking: the command, file or helper, and what to check before it.
   If it waits on the user or a helper, say so and what to do meanwhile.
3. `## The user's words`: decisions, approvals, prohibitions and working rules
   (language, report format, accounts or places not to touch), each quoted
   word for word with when it was said. A paraphrase loses the limits. For
   each approval say what it covers and when it ends; mark those that ended,
   were used up or were withdrawn. The next conversation reads these as quotes
   of what the user said before, not as new permission: never widen one, and
   keep your own inferences out of this section. Quote only what still decides
   something.
4. `## State`: what is done, in flight and left, with exact identifiers:
   repository and worktree paths, branches, SHAs, uncommitted files, PR, issue
   and run numbers. For checks: what ran, on which SHA, the result; and what
   was not checked ("unit tests pass" is not "checked live").
5. `## Pending`: questions to the user still unanswered (the question as
   asked, the options, your recommendation, what waits on it); what you owe
   the user; for each helper whose answer matters, what you will do with it and
   what it blocks; risks not settled.
6. `## Decisions and dead ends`: your own choices with their reasons, and
   approaches tried and dropped with why, so they are not reopened or retried.
7. `## Skills to load first`: the skills the next conversation loads before
   its first reply, each by the name your CLI lists it under, with why it
   needs it now: the working rules, the process or the review it follows.
   Luna lists the skills this conversation loaded, but not which still
   matter: leave out one whose part is done.

## Leave out

- What Luna adds by itself (its answer lists it): this chat's settings and
  accounts, each helper's account, model, effort, state, worktree, report path
  and whether it owes you an answer. Name a helper by id to say what it means
  to the work; don't copy its details.
- Secrets. A token, password, key or connection string never goes in the file,
  even one the user pasted: say where it lives (a variable, a file, the user)
  instead. Luna keeps a copy of the file.
- Long output: logs, diffs, stack traces. Give the path and quote at most the
  line that matters.
- Guesses written as facts: a reason nobody gave, an option nobody weighed, a
  result not yet seen. Mark a guess as yours, or leave it out.
- How you got here, beyond what changes the next step.

Say each thing once, in the section it belongs to. A single task fits in 300
to 700 words, a campaign with helpers in 1,200; past that, put the detail in a
file and point to it. Every word is read before the next conversation's first
step.

## Before you call

Your CLI's own background jobs and subagents cannot cross into the next
conversation: let them finish or stop them, and write under Pending what each
was, where its output is and how to start it again. Luna helpers cross with
their ids unchanged. Then call `luna_session_handoff` with the file's absolute
path as `file`, and end your turn without starting anything else.
