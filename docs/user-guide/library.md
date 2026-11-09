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

## Chats, helpers or both

Each skill and instructions file is for your own chats, for helpers (the
sessions another session starts and briefs, through Luna's tools), or for both:
the **For** column, **Both** until you pick another. A helper is briefed by an
agent rather than by you, so what you wrote for your own chat (the language you
want answers in, how replies to you look, agreeing a plan with you first) and a
skill only your chat uses cost it context and mislead it.

- **Claude Code** leaves an entry out of each session of the other kind at its
  start: instructions through `claudeMdExcludes`, skills through
  `skillOverrides`, both in the settings Luna starts it with. The account's
  files stay as they are.
- **Codex** turns a skill off for one run (`-c skills.config`), but reads its
  account's `AGENTS.md` in every session with no way to leave it out of one. So
  instructions for one kind are laid out with a line before them saying whose
  they are ("For the person's own chats only: …" or "For helper sessions only:
  …"), and Luna's note to every helper says it is a helper. That line goes into
  the Claude Code copy too, for sessions Luna does not start; changing the
  audience rewrites the copies, and taking a copy into the library takes off
  the line Luna wrote there (even if an editor changed its line endings), and
  nothing else: the same words in a copy Luna wrote no line into stay yours.
- An account gets one instructions file, so instructions for your chats only
  leave a Claude Code helper with none from the library; a Codex helper reads
  them, worded as yours.
- **What Luna applied** for a session lists what the audience kept from it
  (or, on Codex, which instructions say they are for the other kind) under
  your choices. Where it could not keep them out it says so: a Codex copy
  changed there, left out or not yet written does not say whose it is; and in
  the rare project whose folder name Claude Code's matcher would confuse with
  the account's, the account's `CLAUDE.md` is read after all.

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

## System skills

One text in the library is no skill of yours to lay out but Luna's own, which
you may override: **`session-handoff`**, what a chat with
[Auto-handoff](chats.md#auto-handoff) on is given when it writes its handoff.
Luna serves it only to such chats, through their `luna_session_handoff` tool,
once they are asked for a handoff; it is never put into an account or a folder,
and no other session, helper or chat without the tick sees it.

The window's **System skills** section, below your skills and set apart from
them, has it on a row of its own: what it is, whose text is served now, and
what can be done about it. It has no accounts, no **For** column and no
folders: there is one text for every chat it is served to.

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
