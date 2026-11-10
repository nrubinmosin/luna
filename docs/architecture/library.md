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
- Never overwrite or take out a copy that changed where it lies: report it.

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
| `library/system.rs` | the system skills: the handoff's instructions resolved, Luna's own or the override in `system/` ([auto-handoff](handoff.md#the-instructions)); the override read, written, compared, sent to the backups or made from a library skill of the name; the reserved name |
| `src/features/library/` | the Skills & instructions window, grids, `ReachPicker`, `AudiencePicker`, `FolderCell`, `LibraryEditor`, `SystemSkills`, `diff.ts` |

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
| no longer wanted, equal to the ledger | taken out |
| no longer wanted, different from the ledger | `kept`: reported on every pass, never taken out unasked |

The window resolves each: take it into the library, put the library's back, or
skip that destination (`library.skipped`). `library_put_back` puts the
library's back at a list of destinations under one lock and one pass after:
the window's "Put all back" (every `gone`) and "Sync all" (every `gone`,
`changed` and `taken`). It writes over what is there only at the
destinations in `replace`, the `changed` and `taken` ones "Sync all"'s
confirmation counted; everywhere else only where nothing is there under the
lock, so a deleted copy made again since the window looked is not overwritten
unasked, even in a batch that replaces others. It writes under the destination as the pass spells it now
(the ledger's key), refuses a skipped destination and goes on past one that
will not settle, returning those in the overview's `unsettled`. The
losing side goes to `<data>/library-backups/` (swept after 30 days), each
backup in a folder of its own: `backup_slot` makes it exclusively, with a
count after a name taken, so two accounts' `skills/review` backed up in one
millisecond do not share one. A copy no longer wanted is
deleted only while it still hashes to the ledger.

A copy no longer wanted that changed there leaves `written` for the ledger's
`kept` (destination → what Luna last wrote there; its blobs stay for the
differences, which `library_diff` shows against them). Every pass, acting or
only looking, reports each one still there and still changed in the
overview's `kept`, with the account or folder its path says it was for. Its
record goes when the file is gone, when its place is left out, when the place
is wanted again (its hashes go back to `written`, so it is Luna's copy, changed
there, under the ordinary statuses), and when it is settled; one that is again
exactly what Luna wrote is taken out by the next acting pass, as any copy no
longer wanted. `library_kept(dests, take_out)` settles a list under one lock
and one pass after: `take_out` backs what is there up and removes it,
otherwise Luna forgets it and the file is the user's own. It finds a record by
`ident`, refuses a place left out or wanted again and goes on past one that
will not settle, as `library_put_back` does. "Sync all" calls it for every kept
copy before its put-back, counting them in its confirmation.

Passes run when the window starts, before every session start, after every
change in the window and on Sync all, one at a time under a lock file beside the
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
- No link or junction stands between a copy and the folder it lies in
  (`Unit::top`, `top_of`: the deepest account folder holding it, else Luna's
  data folder, so a Claude Code folder copy's `library-folders/` is looked at
  too, being Luna's own; a Codex folder copy lies in its account's plugin
  cache, every folder of which is below the account's; a copy an earlier Luna
laid into a listed folder itself, that folder). The pass's writes and
  its removals (a copy no longer wanted, a kept one changed back, both still
  exactly what Luna wrote: equal bytes say the content is Luna's, not the
  place), `put_back` (a resolve, Put all back, Sync all) and `library_kept`'s
  take-out look at every folder below that one on the way down
  (`no_link_between`, on `boundary::walk_down`) and refuse, naming the link;
  the account folder itself may be the person's link. A removal refused keeps
  its ledger note, `written` or `kept`, for a later pass, and is logged once a
  run.
- `system/` is read only for what Luna serves (`library/system.rs`): no pass
  lays anything of it out, and a skill named `session-handoff` under
  `skills/` is not the handoff's override.
- A system skill's name is reserved (`system.rs` `reserved`): a library skill
  of that name is `problem`ed in the overview, wanted nowhere (`units` skips
  it, so a copy an earlier Luna laid out goes as one no longer wanted, unless
  changed there) and hidden from no session (`hidden_here` skips it), so its
  name reaches no account, folder or start. `library_new_skill` refuses the
  name. Laid out, it would tell every session of the handoff.
- The window's System skills section reaches only `system/<name>` for a name
  `luna_text` knows, and Luna's backups: the editor through `library_read` and
  `library_write` with `{"system": name}` (Luna's own text where there is no
  override; a save over 64 KB refused, a draft that will not do saved and
  served as broken), `library_check_system`, `library_system_compare`,
  `library_system_reset` and `library_system_adopt`. The override's SKILL.md
  is opened as `read_override` opens it (as itself, to a cap: 1 MB for the
  editor).
  - **Where:** every path is `inside` the library: no link, junction or file
    in a folder's place anywhere below its folder (`skills/`, `system/`, the
    override's folder, the skill to adopt), and what is there resolves inside
    the library's folder as resolved (`inside`, on `boundary::walk_down`). The
    library's folder itself is where the person put it, a link of theirs if
    they chose one, and theirs, so only looked at. `library-backups/`, Luna's
    own, is checked as the handoffs' folders are (`boundary::private_chain`: no
    link, this user's, private) before anything moves. Anything but plain files
    and folders in a folder to be moved
    (`plain_tree`) is refused too: a FIFO would hold a copy, and the library's
    lock, for good.
  - **Back to Luna's** moves the override's folder whole into
    `library-backups/<stamp>-system-<name>/` (`move_to_backups`, `from.txt`
    beside it): a rename, so its temp files and a save made meanwhile go with
    it. Across volumes the folder is first renamed aside in `system/`
    (`.<name>.leaving-…`, read by nothing, so a save by its name lands where
    it lay), copied from there, and taken out only once the copy holds the
    same files with the same SHA-256 (`tree`); otherwise the copy goes and the
    folder stays aside (`Moved::Aside`). The overview names every such folder
    (`set_aside`, `systemAside`), an adoption's replaced one that would not
    move too, for the person to move or delete.
  - **Adoption** renames the skill into `system/` under a name nothing reads
    (the step that fails where the folder is held open), then, holding the
    system lock exclusive, renames an override there aside and the staged one
    into place, each undone if the next fails; the one set aside goes to the
    backups after. `resolve_in` holds the same lock (`<data>/library-system.lock`,
    a file lock: the window adopts, the core serves) shared while it reads, so
    no handoff is served Luna's text in the moment between the renames.

## Failure modes

| Failure | Behavior |
| --- | --- |
| A file held open mid-save | that unit is `failed` for the pass; copies stay |
| A copy edited in place (by hand or a model) | `changed`, shown under "Needs a look", never overwritten |
| A copy edited in place, then no longer wanted there | `kept`, shown under "Needs a look" until taken out or kept |
| Settings unreadable | no pass (`get_checked`) |
| Sessions started outside Luna | see the account's copies only, not folder skills |
