# Library: skills and instructions

The user's own instructions files and skills, which belong to them rather than
to any repository, laid out into the accounts and folders each is meant for.
User-facing behavior: [user guide](../user-guide/library.md).

## Responsibility

- Keep the library (`<data>/library` or a folder the user picks) as the source
  of truth: `instructions/<name>.md` and `skills/<name>/SKILL.md`.
- Lay copies out into accounts (`CLAUDE.md` / `AGENTS.md`, `skills/<name>`).
- Give folders' sessions extra skills, hidden skills or other instructions at
  every start, writing nothing into the folder.
- Never overwrite a copy that changed where it lies: report it.

## Key modules

| Path | What it holds |
| --- | --- |
| `library/mod.rs` | the library's folders, the ledger, the pass lock |
| `library/reach.rs` | whom a skill or instructions file is for |
| `library/audience.rs` | whose sessions one is for (chats, helpers, both); the wording of a copy for one kind |
| `library/skills.rs` | the library's own files, read and checked |
| `library/files.rs` | copies on disk, blobs, backups |
| `library/folders.rs` | what waits for a named folder's sessions; `session_args` |
| `library/pass.rs` | one pass over every copy |
| `library/overview.rs` | what the window is shown |
| `library/commands.rs` | the window's commands |
| `src/features/library/` | the Skills & instructions window, grids, `ReachPicker`, `AudiencePicker`, `FolderCell`, `LibraryEditor`, `diff.ts` |

## Reaches

Each skill and instructions file has a **reach** in `settings.json`: `all`,
`claude` / `codex` (a provider's every account), or `provider/name` (the key
`agents/` uses too). Renames and deletions carry along; an account a reach stops
taking in loses Luna's copy.

An account has one instructions file, so the closest reach decides (`chosen`):
named beats a provider beats `all`; `library.noInstructions` names accounts
that get none. Setting a reach takes each entry from every other instructions
(`give_instructions`), so two never tie. An account's own file where no
instructions go can be taken in as new instructions named after it. The old
single `instructions.md` moves to `instructions/main.md` at the first pass; with
both present neither is touched and every instructions unit is `failed` until
resolved.

## Audiences

Each instructions file and skill has an **audience** in `settings.json`
(`library.audience.instructions` / `.skills`, by name; absent is `both`):
`chats` (the person's own, run in a pane), `helpers` (run headless by
`background/`), or `both` (`library/audience.rs`). Renaming or deleting an entry
carries its audience along. `session_args` takes the start's `SessionKind`
(`Chat` from `pty/start.rs`, `Helper` from `background/command.rs`) and keeps
from it what is not for its kind, in `folder_args`:

- **Claude Code:** the instructions a session would read — a folder's
  (`folder_instructions`), else the account's own (`chosen`) — if not for its
  kind, are none: the account's `CLAUDE.md` goes in `claudeMdExcludes` and
  nothing replaces it. A skill not for its kind is hidden wherever it would
  come from (the account's copy, a folder's) in `skillOverrides`
  (`hidden_here`). No per-launch helper variant of the account's file is
  written: it would be a second copy to keep current, and an account gets one
  instructions file, so a chats-only file leaves a Claude Code helper with no
  library instructions rather than other ones. The account's file is left out
  by its path taken literally (`glob_literal`), and only where that pattern
  cannot also match one of the session's own instruction files in Claude
  Code's matcher (`pty::claude_account_excludes`, through #30's
  `keep_own_memory`): one that could is read after all and said as the
  Workaround `library.account-instructions-kept`.
- **Codex:** skills as above, in the one `-c skills.config`. Its account's
  `AGENTS.md` cannot be left out of one session (0.167.0: the user-instructions
  provider in `codex-rs/codex-home/src/instructions/mod.rs` reads
  `AGENTS.override.md` or `AGENTS.md` from `CODEX_HOME`, and
  `codex-rs/cli/src/main.rs` installs it with no config switch;
  `developer_instructions` only adds, `model_instructions_file` replaces the base
  prompt). So `units` lays instructions for one kind out `worded`: a line before
  them saying whose they are. Claude Code copies get the same line, for the
  sessions Luna does not start. The ledger notes which line Luna wrote at each
  destination (`Ledger::worded`); `library_resolve` taking a copy in strips
  that line alone, while it and the blank line after it are still there word
  for word with either line ending (`unworded`), so the same words in a copy
  Luna wrote no line into stay the person's.
- What the audience decided goes in `SessionSkills::audience` and is applied
  as `library.audience` (a UserAction, ADR-0016) beside the mechanism's own
  items (`library.hidden-skills`, `library.account-instructions-off`). A Codex
  session is said to be told whose its instructions are only when the
  account's `AGENTS.md` opens on that line now (`carries_banner`); a copy
  changed there, skipped or not written is `library.audience-unkept` instead.
- Nothing is written into an account per launch: the wording is part of the
  copy, written by the pass like any other change of the library.

## Folders

A folder the user names gets nothing written into it: a copy there would reach
no worktree (each CLI makes one of what git tracks) and is one `git add` from
the team's repository. Instead, at every start, `session_args`
(`pty/start.rs`, `background/command.rs`) maps the session's folder (through
`worktree::root_of`: `<worktree>/src` is `<project>/src`) onto the named folders
it lies in.

- **Claude Code:** skills wait in
  `<data>/library-folders/<slug>/<account>/.claude/skills/<name>` and are passed
  as `--add-dir` (its `.claude/skills` made before the start, or a skill added
  later never shows). A name the repository, the account or its synced skills
  already have goes as `luna-<name>` (`free_names`, `with_name`), since Claude
  Code would otherwise list it scoped to Luna's folder. Such a copy is compared,
  put back and diffed as the renamed skill, and taken into the library under its
  own name again.
- **Codex:** a plugin in the account's `plugins/cache/luna/<slug>/local/`
  (manifest written by Luna, outside the ledger), off until
  `-c plugins.<slug>@luna.enabled=true`; listed as `<slug>:<name>`. The slug is
  `[a-z0-9-]`, no dots: Codex splits a `-c` key at each.
- A folder's own reach per skill (`LibraryFolder::skills`) decides for its
  sessions; the deepest folder with one decides (`deciding`, `gets_in`). A copy
  goes to an account its reach takes in that does not have the skill as a whole
  nor a copy from an outer folder (`folder_copy_for`).
- **Hiding** a skill an account has: Claude Code `skillOverrides: {name: "off"}`
  in the session's single `--settings` file (`claude_settings_file`); Codex
  `-c skills.config=[{name=…, enabled=false}]`. Both hide by name (an outer
  folder's copy as the CLI lists it: `luna-<name>` or `<slug>:<name>`), so a
  repository's skill of the same name goes too. The window shows per cell what a
  folder's sessions would get with no reach of their own (`inherited`).
- Settings from before reaches (`folders_only`, a folder's ticked `skills` and
  `off`) read as reaches: a tick is `all`, or the skill's reach for one that was
  folders only (and then none as a whole); off is none.
- **Instructions per folder**, Claude Code only (`folder_instructions`): the
  account's `CLAUDE.md` in `claudeMdExcludes`, the other file copied at every
  start to `hooks/<id>/CLAUDE.md` and passed as `--add-dir` with
  `CLAUDE_CODE_ADDITIONAL_DIRECTORIES_CLAUDE_MD` (`pty::claude_instructions_dir`):
  memory is read again on resume, unlike `--append-system-prompt-file`. Codex
  reads its account's `AGENTS.override.md` or `AGENTS.md` with no way to turn it
  off, so Codex columns are disabled.

## The pass

The library is the source; each destination holds a copy. A pass compares three
things per destination: the library's files, what is there, and the ledger's
hashes of what Luna wrote last (`<data>/library-state.json`, bytes in
`library-blobs/` by hash).

| On disk vs. library and ledger | Action |
| --- | --- |
| missing | written |
| equal to the ledger, behind the library | rewritten |
| equal to the library, no ledger entry | adopted |
| different from the ledger | `changed` (or `gone`): reported, never written |
| no ledger entry and different | `taken`: reported, never written |

The window resolves each: take it into the library, put the library's back, or
skip that destination (`library.skipped`). The losing side goes to
`<data>/library-backups/` (swept after 30 days). A copy no longer wanted is
deleted only while it still hashes to the ledger.

Passes run when the window starts, before every session start, after every
change in the window and on Sync now, one at a time under a lock file beside the
ledger shared by window and core. Where `lay_out` runs in a start:

- **A chat** (`pty/start.rs`): inside the launch closure, so under that chat's
  own spawn gate (one chat's starts wait on its pass; other chats and their
  input do not). Claude Code: after the worktree fetch, then `session_args`,
  then the settings file and `register`. Codex: after `register`, then
  `session_args`. A login session gets no pass.
- **A background helper's turn** (`background/run.rs`): before the background
  registry is locked for the command line, since every helper's events wait on
  that lock and a pass reads and writes every account's copies. Window commands run off its main thread (`command(async)`).
The window re-reads the overview (no writes) on every focus.

## Invariants

- A pass goes only by settings as the file has them now
  (`settings::get_checked`): on defaults it would take every copy out.
- A skill whose `SKILL.md` lacks front matter with this `name` and a
  `description` is `failed`; its copies stay. So is everything fed by a
  `skills/` that will not list or an instructions file that will not read.
- A library folder that is not there wants nothing and touches nothing.
- Paths compare as the file system does (`ident`: on Windows either slash, any
  case).
- A destination inside the library's `skills/`, or one holding the library, is
  `failed`: never written, adopted or removed.
- Writes go through `<file>.<pid>-<n>.luna-tmp`; links are never followed or
  written over.

## Failure modes

| Failure | Behavior |
| --- | --- |
| A file held open mid-save | that unit is `failed` for the pass; copies stay |
| A copy edited in place (by hand or a model) | `changed`, shown under "Needs a look", never overwritten |
| Settings unreadable | no pass (`get_checked`) |
| Sessions started outside Luna | see the account's copies only, not folder skills |
