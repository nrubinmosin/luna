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
  for an account.
- **Instructions for your chats only** that an earlier Luna put into accounts
  stay there, with a line before them saying whose they are ("For chats with a
  person only: a session whose task came from another agent skips this whole
  file."), so that chats you start by hand still read them. Codex reads its
  account's `AGENTS.md` in every session with no way to leave it out of one;
  the line, and the note on a lead's or helper's task, tell such a session it
  is not for it. Claude Code leaves the file out of a lead or helper
  (`claudeMdExcludes`). Turning any other role on for such a file takes it out
  of the accounts; from then on it goes per session.
- **What Luna applied** for a session lists what the audience gave it or kept
  from it. Where it could not keep something out it says so: a Codex copy
  changed there, left out or not yet written does not say whose it is; and in
  the rare project whose folder name Claude Code's matcher would confuse with
  the account's, the account's `CLAUDE.md` is read after all.

## What a session is told of its role

Beside its account's instructions file, every session Luna starts is told of
its role in layers of its own, at the level of an instructions file, which
outweighs a note on a prompt and survives a compaction and a handoff:

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
a command line (24 KB) starts without them, and its panel says so; a lead or
helper is not started. The same goes for a Claude Code session whose file Luna
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

The library is the original and every place holds a copy. Luna remembers what it
wrote where, so it touches only its own copies: anything else stays, and a copy
no longer wanted goes only while it is still exactly what Luna wrote.

A copy changed or deleted where it lies (by hand, or by a model improving a skill
in place) is never silently replaced: it shows under **Needs a look**, and as a
chip on the status bar, with its differences. Take it into the library and it
becomes every copy's; put the library's back; or leave that place out. The side
replaced is kept in Luna's backups for 30 days.

A copy no longer meant for its place (a skill un-ticked for an account, say)
goes, but only while it is still exactly what Luna wrote. One changed there
since stays, and waits under Needs a look, "no longer meant for it, changed
there", with its differences from what Luna wrote: **Take out** removes it,
kept in Luna's backups for 30 days; **Keep** leaves it as your own file, and
Luna forgets it was ever its copy. Picked for that place again, it is Luna's
changed copy there once more; deleted, or changed back to just what Luna
wrote, it no longer waits.

With several copies deleted, **Put all back** above the list puts the library's
back in each of them; the changed ones still wait there. **Sync all**, at the
top of the window, makes every account hold what the library says: deleted
copies put back, changed ones replaced and those no longer meant for their
place taken out, after a confirmation that counts them. It goes by what the grids pick and leaves the places left out as they
are; a copy that cannot be laid out (a skill whose `SKILL.md` does not do) stays
under Needs a look.

## When it is laid out

When Luna starts, before every session Luna starts (a chat, a resume, each turn
of a helper), on every change made in the window and on Sync all, so a file
changed in the library by another program reaches the accounts by the next
session. It is looked at again whenever the window comes to the front.

A skill whose `SKILL.md` does not do (no front matter, a name other than its
folder's) is laid out nowhere until it does, and its copies stay as they were;
so do those of a library that will not read for a moment (a file held open
mid-save). Paths compare as the system compares them (on Windows regardless of
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
- **`session-handoff`**, described below.

What stays Luna's own and is not a text you can change: its tools' schemas and
refusals, the facts of its guide, the tag a note opens on.

### The handoff's skill

**`session-handoff`** is what a chat with Luna tools is given
when it writes its [handoff](chats.md#handoff), or a move's document (under
the move's own rules). Luna serves it only to such chats, through their
`luna_session_handoff` tool, once they are asked for a handoff or a move's
document; it is never put into an account or a folder, and no helper or other
session sees it.

The window's **System texts** section, below your skills and set apart from
them, has each system text on a row of its own: what it is, whose text is
served now, and what can be done about it. None has accounts, a **For** column
or folders: there is one text for every session it is served to. **Edit**,
**Compare with Luna's** and **Back to Luna's** below work the same for every
one of them, each checked as its kind is.

- **Luna's own** is built in, of its version: the row says which, and the
  first 12 characters of the text's SHA-256.
- **Edit** opens the editor on your override, `system/session-handoff/SKILL.md`
  in the library; where there is none yet it starts from Luna's text, and
  saving makes it yours. It needs front matter with `name: session-handoff` and
  a `description`, then the text, at most 64 KB; the editor checks it as you
  type, as Luna checks it before serving it. The row then says **Yours**, with
  its path and hash. Only that file is read and served; files beside it in its
  folder are not, though the text may name them: the chat is told where they
  are.
- **Yours is broken** when it does not do (no front matter, another name, no
  description, nothing past the front matter, a link, a file not of your own making, too large): it is saved
  all the same, but Luna's own is served in its place, and the row and the
  chat's **What Luna applied** panel say why. A file over 64 KB is not saved
  from the editor at all.
- **Compare with Luna's** shows your text against Luna's own, line by line:
  after an update, what Luna's own has that yours lacks.
- **Back to Luna's** asks first, then moves your override's folder into Luna's
  backups, kept 30 days like any file the library replaces. Where the backups
  are on another drive than the library, it is copied there and checked; if a
  save changed it meanwhile, it is kept aside in `system/` instead, not served,
  and the section names where, for you to move or delete.
- A skill named `session-handoff` among your skills is not the override, and
  it is laid out nowhere: in every account and folder it would tell every
  session of the handoff. Its row says so and offers **Make it the override…**,
  which asks first and then moves its folder, `SKILL.md` and all beside it, to
  `system/session-handoff`; an override already there goes to the backups. A
  copy of it laid out by an earlier Luna is taken out, unless changed there
  since. No new skill takes the name.
- A `session-handoff` skill in a project's `.claude/skills` or `.agents/skills`
  or an account's `skills/` is not the override either: Luna never serves those
  for a handoff, though the CLI may list them to the model. The chat's panel
  warns of one in its folder or account.
