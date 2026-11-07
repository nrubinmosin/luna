# Skills and instructions

Luna keeps a library of your own: instructions files, each with a name, and any
number of skills, in a folder of its own (`library` in Luna's data folder) or
one you choose, such as a repository of your dotfiles. The **Skills &
instructions** window, from the Accounts window, makes, renames and edits them
and says where each goes. Instructions are told apart by their first heading;
each name has a copy button, to name a skill or instructions in a chat.

## Where things go

- Each skill and instructions file goes into the accounts you pick: every
  account, every account of one CLI, or accounts by name. A renamed account keeps
  what was picked for it.
- **Instructions** go in as `CLAUDE.md` in a Claude Code account, `AGENTS.md` in
  a Codex one: the files each CLI reads at the start of every session in every
  project. One file serves both CLIs, and an account gets one at most (a radio
  per account in the window). Where several take an account in, the one naming
  it wins over one for its CLI, and that over one for every account, so `main`
  for everyone and `work` for the work account just works.
- An account that already has its own file keeps it until you take it into the
  library or replace it; one that gets nothing from the library can have its own
  taken in as new instructions named after it. The single `instructions.md` of
  earlier versions becomes `instructions/main.md`.
- A Codex account with an `AGENTS.override.md` reads that instead of
  `AGENTS.md`; the window says so.
- A **skill** is a folder with a `SKILL.md` (front matter with its `name` and a
  `description`, then the instructions) and any files beside it. It goes into
  each picked account's `skills/`, for every session in every project.

## Folders

A folder you add (a repository, say) is for what belongs to one project but not
to everyone who works on it. "Add a folder…" lists the new-chat dialog's folders
and any other through the picker; one holding several repositories gives its
skills to each.

- **Skills per folder:** for each skill a folder can pick the accounts whose
  sessions there get it: every account, a CLI's, some by name, or none. The
  sessions Luna starts there (worktrees, helpers, resumes and the CLIs' own
  subagents included) get it at their start. Nothing is written into the folder,
  so nothing shows in `git status` and nothing is missing from a fresh worktree.
  Claude Code lists them by their own names (one whose name the repository or the
  account already has goes as `luna-<name>`); Codex gets them as a plugin and
  lists them as `<folder>:<name>`. Sessions started outside Luna do not see them.
- **Hiding:** a skill a folder keeps from an account that has it as a whole is
  hidden from the session at its start; the account's copy stays. Both CLIs hide
  by name, so one of the same name the repository has goes too. Where one folder
  lies in another, the inner one decides.
- **Instructions per folder** (Claude Code only): a folder can give its sessions
  other instructions than their account's, or none, read again at every start,
  a resume's too. Codex reads its account's `AGENTS.md` in every session with no
  way to leave it out, so a folder does not change a Codex account's
  instructions.
- So every combination holds: a skill on one account everywhere and on another
  only in one repository; on every account but kept out of one folder; only in
  one folder and only on Claude Code there.
- In the grid a tick is every account, a dash some, a red cross none; a faint
  tick or dash is what the folder's sessions get with nothing picked there. A
  click opens the accounts, with "As outside this folder" to pick nothing.

## Copies and changes

The library is the original and every place holds a copy. Luna remembers what it
wrote where, so it touches only its own copies: anything else stays, and a copy
no longer wanted goes only while it is still exactly what Luna wrote.

A copy changed or deleted where it lies (by hand, or by a model improving a skill
in place) is never silently replaced: it shows under **Needs a look**, and as a
chip on the status bar, with its differences. Take it into the library and it
becomes every copy's; put the library's back; or leave that place out. The side
replaced is kept in Luna's backups for 30 days.

## When it is laid out

When Luna starts, before every session Luna starts (a chat, a resume, each turn
of a helper), on every change made in the window and on Sync now, so a file
changed in the library by another program reaches the accounts by the next
session. It is looked at again whenever the window comes to the front.

A skill whose `SKILL.md` does not do (no front matter, a name other than its
folder's) is laid out nowhere until it does, and its copies stay as they were;
so do those of a library that will not read for a moment (a file held open
mid-save). Paths compare as the system compares them (on Windows regardless of
case), so an account renamed in case only keeps its copies.
