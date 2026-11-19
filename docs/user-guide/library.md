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
- An account that already has its own file keeps it: Luna never writes over a
  file it did not write. One that gets nothing from the library can have its own
  taken in as new instructions named after it. The single `instructions.md` of
  earlier versions becomes `instructions/main.md`.
- A Codex account with an `AGENTS.override.md` reads that instead of
  `AGENTS.md`; the window says so.
- A **skill** is a folder with a `SKILL.md` and any files beside it. It goes
  into each picked account's `skills/`, for every session in every project.
  Luna lays it out as it is and does not judge what `SKILL.md` says (its front
  matter, its `name`, how long its description is): that is up to Claude Code
  and Codex, which take or leave the skill as they see fit. Luna asks only
  that the folder reads, holds a `SKILL.md`, and is named what a folder can be
  called on any system (no `/`, `\` or `:`, nothing Windows keeps for itself
  such as `con` or `nul`). The description the window shows under a skill's
  name is read from the front matter when there is one, and left empty when
  there is not.

## Whose sessions: chats, leads, helpers

Luna starts sessions in three roles: your own **chats**; **leads**, the
helpers another session starts with Luna's tools, which may start helpers of
their own; and **helpers**, started without them. A lead or a helper is
briefed by an agent rather than by you, so what you wrote for your own chat
(the language you want answers in, how replies to you look, agreeing a plan
with you first) and a skill only your chat uses cost it context and mislead
it, and a lead needs rules a helper does not.

Each skill and instructions file has a **For** column of three toggles,
**Chats**, **Leads** and **Helpers**, all on (every session) until you turn
one off. At least one stays on.

- **Skills** go into the accounts they are for as ever. Claude Code leaves one
  out of a session whose role is not ticked (`skillOverrides` in the settings
  Luna starts it with); Codex turns it off for that run (`-c skills.config`).
- **Instructions for every session** go into the accounts as `CLAUDE.md` and
  `AGENTS.md`, one per account, as described above.
- **Instructions for some roles only** are never put into an account. Luna
  gives them to each session of those roles as it starts it, along with what
  it tells the session of its role ([below](#what-a-session-is-told-of-its-role)):
  every such file whose accounts take the session's account, in name order.
  They reach only sessions Luna starts, not one you start by hand. Their row
  has a box per account rather than a radio: they compete with no other file
  for an account. Turning all three roles back on is refused, naming the other
  file, while other files for every session (or **no instructions**) hold
  every account one of its choices takes in, as closely or more (an account
  by name beats a provider, a provider beats **all**, and Claude Code and
  Codex together hold all): only one could go into the account's file, and
  this one would reach nobody there. Change either one's accounts first.
- **Instructions for your chats only** that an earlier Luna put into accounts
  stay there, with a line before them saying whose they are ("For chats with a
  person only: a session whose task came from another agent skips this whole
  file."), so that chats you start by hand still read them. Codex reads its
  account's `AGENTS.md` in every session with no way to leave it out of one;
  the line, and the note on a lead's or helper's task, tell such a session it
  is not for it. Claude Code leaves the file out of a lead or helper
  (`claudeMdExcludes`). Turning any other role on for such a file takes it out
  of the accounts; from then on it goes per session.
- A session's start records what the audience gave it or kept from it
  (`library.audience`). Where it could not keep something out it says so: a Codex copy
  changed there, left out or not yet written does not say whose it is; and in
  the rare project whose folder name Claude Code's matcher would confuse with
  the account's, the account's `CLAUDE.md` is read after all.

## What a session is told of its role

Beside its account's instructions file, every session Luna starts is told of
its role in layers of its own, at the level of an instructions file, which
outweighs a note on a prompt and survives a compaction:

1. what Luna says of its role, a [system text](#system-texts) you may change:
   nothing for a chat, unless you write something; for a lead or a helper,
   that its task comes from another agent, to answer it in the language and
   form the task asks for, to stop background work nothing needs, to keep
   caveats beside their claims, to send nothing outward and to change nothing
   machine-wide unless the task says so; a lead also that it owns what its
   helpers report;
2. the library's instructions for its role, in name order (in a folder that
   gives a Claude Code session other instructions than its account's, those
   come first);
3. its **constraints**: what the agents above it asked of every session under
   them when they spawned it, word for word ([agents](agents.md#spawning-options)).

Claude Code reads them as the `CLAUDE.md` of a folder of Luna's, written again
at every start and every turn of a helper, so a change reaches a session at its
next turn. Codex reads them in `developer_instructions`, after the value its
own config sets; Codex sends that when a conversation starts and after a
compaction, not on a resume, so a change reaches a running Codex session only
then. A Codex chat whose layers, with its config's own value, would not fit on
a command line starts without them, and its panel says so; a lead or helper is
not started. The Library window marks each instructions file for some roles
that, with the others for the same role, would not fit a Codex account's
command line. The same goes for a Claude Code session whose file Luna
cannot write (a locked file, a full disk): a chat starts without its layers and
says so, a lead's or helper's turn is not started.

A lead's or helper's task also carries a one-line note after it: that it comes
from another agent, not a person, and that rules written for a chat with a
person do not apply. The note is a system text too.
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

The library is the source of truth and every place holds a copy. Luna remembers
what it wrote where, and only that makes a copy Luna's: anything else in a
place, a file of yours or of a CLI's that Luna did not write, stays as it is,
and the library's copy does not go there while it is. Such a place says so in
the window; it clears once the file is gone (the library's copy then goes
there) or once you leave the place out.

Luna's own copies always end up as the library has them. One changed where it
lies has the library's written over it the next time the library is laid out,
which is before every session Luna starts; one deleted there is put back; one
no longer meant for its place (a skill un-ticked for an account, say) is taken
out, changed there or not. What a change there had made is kept in Luna's
backups (`library-backups` in its data folder) for 30 days, and Luna's log names
each place written over or taken out and where its backup is.

**Edit in the library, not in an account.** An edit made inside an account to
something Luna laid out there is undone at the next session start, a backup of
it kept for 30 days: Claude Code's `/memory` on the account's `CLAUDE.md`, a
model improving a skill in place with skill-creator, a hand edit of an
account's `AGENTS.md` or `skills/<name>/SKILL.md`. Make the change in the
library's own file (the editor here, or the file under the library's folder)
and every copy follows. A place left out is no longer Luna's: whatever is there
stays.

In a skill's folder only the files Luna wrote are Luna's. Anything a person or
a tool adds beside them (a `__pycache__/`, a `node_modules/`, a `.DS_Store`, a
file of your own) is not looked at, backed up or removed; when the skill is no
longer meant for that place, Luna's files go and the folder stays with the
rest in it. A copy that changes while the library is being laid out (a CLI
saving it at that moment) is left until the next time.

A copy an earlier Luna laid into one of the library's folders itself, changed
there since, is that folder's own and stays. An account Luna does not list for
the moment keeps its copies as they are until it is back.

**From 0.19.** The first time 0.20 lays the library out, every copy that waited
under 0.19's Needs a look as changed there has the library's written over it
(the change in the backups), and every one deleted there comes back. Copies
0.19 kept in place as no longer wanted but changed there are yours from 0.20 on:
they stay where they are, Luna never takes them back even if they come to hold
just what the library has, and the log names them once.

**Sync all**, at the top of the window, lays the library out at once. A copy
that cannot be laid out (a skill folder with no `SKILL.md`, or one that will
not read) stays as it is until it can.

## When it is laid out

When Luna starts, before every session Luna starts (a chat, a resume, each turn
of a helper), on every change made in the window and on Sync all, so a file
changed in the library by another program reaches the accounts by the next
session. It is looked at again whenever the window comes to the front.

A skill folder with no `SKILL.md`, or one that will not read, is laid out
nowhere until it does, and its copies stay as they were; so do those of a
library that will not read for a moment (a file held open mid-save). Paths compare as the system compares them (on Windows regardless of
case), so an account renamed in case only keeps its copies.

## System texts

Some texts in the library are no entries of yours to lay out but Luna's own,
which you may override, each with one file for every session it is served to:

- **`role-chat`**, **`role-lead`**, **`role-helper`**: what a session of each
  role is told of it ([above](#what-a-session-is-told-of-its-role)). Luna's own
  for a chat is empty. Your override is `system/<name>/TEXT.md`, plain
  Markdown with no front matter, at most 8 KB; an empty one says nothing.
- **`note-lead`**, **`note-helper`**: the note after a task another agent gives
  a lead or a helper. Your override is `system/<name>/TEXT.md`: one line of
  plain text, at most 1,000 characters, with no brackets (the note goes inside
  brackets of its own). Luna remembers every note it has served, so none ever
  shows in what a session was asked, after an update or a change of yours.

What stays Luna's own and is not a text you can change: its tools' schemas and
refusals, the facts of its guide, the tag a note opens on.

The window's **System texts** section, below your skills and set apart from
them, has each system text on a row of its own: what it is, whose text is
served now, and what can be done about it. None has accounts, a **For** column
or folders: there is one text for every session it is served to. **Edit**,
**Compare with Luna's** and **Back to Luna's** below work the same for every
one of them, each checked as its kind is.

- **Luna's own** is built in, of its version: the row says which, and the
  first 12 characters of the text's SHA-256.
- **Edit** opens the editor on your override, `system/<name>/TEXT.md` in the
  library; where there is none yet it starts from Luna's text, and saving
  makes it yours. The editor checks it as you type, as Luna checks it before
  serving it. The row then says **Yours**, with its path and hash. Only that
  file is read and served.
- **Yours is broken** when it does not do (too large, not UTF-8 text, a note
  of more than one line or with a bracket, a link, a file not of your own
  making): it is saved all the same, but Luna's own is served in its place,
  and the row says why. A file over its cap is not saved from the editor at
  all.
- **Compare with Luna's** shows your text against Luna's own, line by line:
  after an update, what Luna's own has that yours lacks.
- **Back to Luna's** asks first, then moves your override's folder into Luna's
  backups, kept 30 days like any file the library replaces. Where the backups
  are on another drive than the library, it is copied there and checked; if a
  save changed it meanwhile, it is kept aside in `system/` instead, not served,
  and the section names where, for you to move or delete.
## In the native window

Release 0.20's native window has the library as a window over the main one
(the title bar's Menu, Library), laid out as a list and the entry picked
rather than as one grid:

- **Across the top:** the library's folder with Open, Change… and Default
  (with unsaved text in the editor, either asks first; the new folder opens
  with nothing picked), and Sync, which lays the library out now, as Sync
  all does ([above](#copies-and-changes)): Luna's own copies are written
  over with the library's or put back, those no longer wanted taken out, a
  changed one backed up first, and a file Luna did not write is left alone.
  It says how many copies it wrote and took out, how many places it left
  alone as not Luna's, and each copy it could not write or take out, with
  why (Syncing…, Synced, or why it failed).
- **On the left:** the instructions (and "(no instructions)", the accounts
  that get none), the skills, Luna's system texts and the folders, each with
  a field and New, or Add folder…. An entry that will not do is marked `!`, a
  system text of yours `yours`, a folder that is not there `gone`.
- **On the right, the entry picked:** Rename and Delete… (a skill can be
  renamed too: its folder, its SKILL.md's `name` and everything the library
  says of it follow, and its copies under the old name go; a rename that
  fails partway is undone, and if even that fails the window says what is
  left where); whom it is meant
  for (the same ticks as before); whose sessions it is for (Chats, Leads,
  Helpers); for instructions, which instructions each account gets, one at
  most; for a skill, which accounts' sessions get it in each folder; for a
  folder, the instructions its sessions on each Claude Code account get in
  place of the account's; for a system text, Compare with Luna's (Luna's own
  text shown beside yours, no diff) and Back to Luna's….
- **Under it, the editor:** the entry's file, Markdown highlighted, with Save
  (Ctrl+S, Cmd+S on macOS). A system text is checked a moment after you stop
  typing and saved even when it does not do yet; a SKILL.md is not checked. Anything that
  would replace or close the editor with unsaved text asks first: picking
  another entry, a rename, Delete…, Back to Luna's…, Add folder…, another
  library folder, closing the window. If the file was changed on disk since
  you opened it (by another editor, say), Save writes nothing and keeps your
  text: Reload takes the file as it is now, Overwrite saves yours over it.
  If the library's folder is no longer the one the file was read from (a
  link there now leads elsewhere, or the folder was made again), nothing is
  saved, Overwrite included: Reload reads the file from the folder there
  now. If the save went through but laying the library out after failed,
  the line under the editor says it was saved and why laying it out did not
  go.
  Unsaved text is kept as you type, so if Luna quits or the window restarts
  from the tray with it unsaved, the library offers it the next time it
  opens: Restore or Throw away. Throw away forgets only that text, never
  text you have typed since in another file.

Not in it any more, as [ADR-0030](../adr/0030-scope-of-0-20.md) has it:
"Needs a look" and its diffs, choosing copy by copy what to put back, keep
or leave out, and taking an account's own file into the library.
The library is the source of truth: what Luna manages in an account is what
the library says.
