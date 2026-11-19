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
- Make every copy Luna's ledger names the library's again (written over, put
  back or taken out, what a change there made backed up first), and never
  write over or take out a file Luna did not write.

## Key modules

| Path | What it holds |
| --- | --- |
| `library/mod.rs` | the library's folders, the ledger, the pass lock |
| `library/reach.rs` | whom a skill or instructions file is for |
| `library/audience.rs` | whose sessions one is for (every session, or some of chat, lead, helper); where each goes; the wording of a copy laid out for the chats |
| `library/role.rs` | what a session is told of its role: its role's system texts, the layers composed, Codex's `developer_instructions`, the notes served |
| `library/skills.rs` | the library's own files, read and checked |
| `library/files.rs` | copies on disk, backups |
| `library/folders.rs` | what waits for a named folder's sessions; `session_args` |
| `library/pass.rs` | one pass over every copy |
| `library/overview.rs` | what the window is shown |
| `library/commands.rs` | the window's commands |
| `library/system.rs` | the system texts (the handoff's skill, the roles' defaults and notes): each resolved, Luna's own or the override in `system/` ([handoff](handoff.md#the-instructions)); the override read, written, compared, sent to the backups or made from a library skill of the name; the reserved name |
| `src/features/library/` | the Skills & instructions window, grids, `ReachPicker`, `AudiencePicker`, `FolderCell`, `LibraryEditor`, `SystemSkills`, `diff.ts` |

## Reaches

Each skill and instructions file has a **reach** in `settings.json`: `all`,
`claude` / `codex` (a provider's every account), or `provider/name` (the key
`agents/` uses too). Renames and deletions carry along; an account a reach stops
taking in loses Luna's copy.

An account's rename carries along, under the pass lock from before its folder
moves to after the settings are written (`rename_account_held`): its reaches,
the ledger's notes of every copy under its folder (`written`, re-keyed by path
prefix, a change of case included, as some lookups take a key as it is), the
places left out there, and for Claude Code its folder
copies, outside the account but named after it: each
`library-folders/<slug>/<name>` is renamed `<slug>/<new>` and its notes follow,
unless a link or junction lies on the way or the new name is taken, when it
stays for a pass to lay out anew and take out. Where no such folder is on disk,
its notes and places left out follow the name all the same. Codex's folder
plugins lie in the account and move with it. Settings that will not write put
the folder copies and the account's folder back, then the ledger as it was, or,
where a folder would not move back, with that folder's notes where it now is;
the error names what stayed where. A deletion, under the same lock, cleans the
settings (reaches, places left out) and then the ledger's notes under the
folder, each step one that a second deletion of a folder already gone
finishes; its folder copies go at the next pass, as copies no longer wanted.

A session's start (a chat's, a helper's turn) counts its account as in use
from before its pass waits for the lock until its process is counted itself
(`lay_out_for`), and a rename asks again under the lock: a rename waiting
behind a start refuses, and a start waiting behind a rename that moved its
account is refused rather than made on the old path.

An account has one instructions file, so the closest reach decides (`chosen`):
named beats a provider beats `all`; `library.noInstructions` names accounts
that get none. Setting a reach takes each entry from every other instructions
(`give_instructions`), so two never tie. An account's own file where no
instructions go can be taken in as new instructions named after it. The old
single `instructions.md` moves to `instructions/main.md` at the first pass; with
both present neither is touched and every instructions unit is `failed` until
resolved.

## Audiences

Luna starts sessions in three roles (`settings::Role`): `chat` (the person's,
run in a pane: `pty/start.rs`), `lead` (a background helper spawned with
`tools`, which may spawn its own: `background/command.rs` `role_of`) and
`helper` (spawned without them). Each instructions file and skill has an
**audience** in `settings.json`, by name (absent is every session), written
so that Luna 0.17 rolled back to still reads the file (`Audiences`'
Serialize): `library.audience.roles.instructions` / `.skills` hold every
audience as a list of roles, which 0.17 ignores as it ignores any key it does
not know; `library.audience.instructions` / `.skills` hold 0.17's word where
it has one (`chats` for `["chat"]`, `helpers` for `["lead", "helper"]`), the
other sets left out, every session's to it. A file with `roles` is read from
there; one without is an earlier Luna's, or one 0.17 saved, and its words read
as roles: `both` is every session (`AudienceField`). Renaming or deleting an
entry carries its audience along. `session_args` takes the start's `Role` and,
in `folder_args`, keeps from it what is not for its role and gives it what is:

- **Where instructions go** (`audience.rs` `laid_into_accounts`): an entry for
  every session is laid into accounts, one per account (`reach.rs` `chosen`,
  which considers only these). An entry for some roles is never laid into an
  account: it reaches each session of its roles as one of its role layers
  (`SessionSkills::role_entries`, every such entry whose reach takes the
  account, in name order; `no_instructions` is about an account's own file and
  does not hold them back), so `give_instructions` makes reaches exclusive only
  among entries laid into accounts. Made every session's, such an entry would
  join that competition with a reach it was never given against them:
  `library_set_audience` refuses it while another laid entry, or
  `no_instructions`, names one of its reach's entries
  (`every_session_refused`), naming the entry and the accounts. The exception is
  `LibrarySettings::laid_out`: entries an earlier Luna laid out for the
  person's chats (seeded from `"chats"` when a file without `roles` is read) stay in the
  accounts with a line before them (`banner_of`, `worded`), so chats started
  outside Luna keep them. Setting such an entry's audience to anything else
  takes it out of `laid_out`, and the next pass takes its copies out as no
  longer wanted. A rename moves its
  `laid_out` with its reach and audience (`rename_instructions`); a delete
  drops it, and new instructions under the name never inherit it
  (`forget_instructions`). An old helpers-only
  entry leaves the accounts the same way; `chosen` may then give an account
  another file, which the pass reports.
- **Claude Code:** the instructions a session would read from its account or
  a folder — a folder's (`folder_instructions`), else the account's own — if
  not for its role, are none: the account's `CLAUDE.md` goes in
  `claudeMdExcludes`. A folder's instructions for its role go first in its role
  layers, in the account's place. A skill not for its role is hidden wherever it
  would come from (the account's copy, a folder's) in `skillOverrides`
  (`hidden_here`). The account's file is left out by its path taken literally
  (`glob_literal`), and only where that pattern cannot also match one of the
  session's own instruction files in Claude Code's matcher
  (`pty::claude_account_excludes`, through #30's `keep_own_memory`): one that
  could is read after all and said as the Workaround
  `library.account-instructions-kept`.
- **Codex:** skills as above, in the one `-c skills.config`. Its account's
  `AGENTS.md` cannot be left out of one session (0.167.2: the app-server
  installs `CodexHomeUserInstructionsProvider` unconditionally, which reads
  `AGENTS.override.md` or `AGENTS.md` from `CODEX_HOME`, an empty override
  skipped; `model_instructions_file` replaces the base prompt). So an entry
  still laid out for the chats goes `worded`: a line before it saying whose it
  is, `FOR_CHATS`. Claude Code copies get the same line, for the sessions Luna
  does not start. A copy says so (`carries_banner`) while that line (this
  Luna's or an earlier one's, `wordings`) and the blank line after it are
  still there word for word with either line ending (`unworded`). A copy
  still opening on an earlier Luna's line is Luna's own, behind, and the next
  pass rewrites it.
- What the audience decided goes in `SessionSkills::audience` and is applied
  as `library.audience` (a UserAction, ADR-0016) beside the mechanism's own
  items (`library.hidden-skills`, `library.account-instructions-off`). A Codex
  session is said to be told whose its instructions are only when the
  account's `AGENTS.md` opens on that line now (`carries_banner`); a copy
  changed there, skipped or not written is `library.audience-unkept` instead.
- Nothing is written into an account per launch: the wording is part of the
  copy, written by the pass like any other change of the library.

## Role layers

What a session is told of its role (ADR-0026, `library/role.rs`), after its
account's file, in this order: Luna's default for its role (the system texts
`role-chat`, empty, `role-lead`, `role-helper`), the library's entries for its
role (`role_entries`; a Claude Code folder's instructions first, in the
account's place), then its constraints (agents/spawn `Constraint`), each under
a heading of its own (`compose_layers`). Nothing at all is none: a chat with
no override and no entries for chats is started as before.

- **Claude Code:** written at every start and every helper turn to
  `hooks/<id>/CLAUDE.md` (owner-only, `secret::write_private`) and passed as
  `--add-dir` with `CLAUDE_CODE_ADDITIONAL_DIRECTORIES_CLAUDE_MD`
  (`pty::claude_instructions_dir`); the other added folders' memory is left out
  (`claude_memory_off`). Memory is read again on resume, unlike
  `--append-system-prompt`, which Luna no longer passes. Where the file will
  not write (a locked file, a full disk), a lead's or helper's turn is refused
  rather than run without its role and constraints; a chat starts without its
  layers and says so (`role.layers-off`).
- **Codex:** `-c developer_instructions=<value>` (`pty::codex_role_args`), the
  value being what the config stack sets (`codex/config.rs`
  `developer_instructions_in`: `/etc/codex/config.toml` on Unix, the account's
  `config.toml`, a trusted project's layers) and then the layers. A `-c`
  replaces the stack's string, so Luna puts it first. No profile goes over it:
  0.167.2 refuses a legacy `profile = "<name>"` selector, and a `--profile`
  file is a layer below the `-c` (docs/cli-compatibility.md). A
  managed layer, which comes above the `-c`, that sets the key, or a layer Luna
  cannot read, is said as `role.layers-unsure`; the session starts all the
  same. The `developer_instructions=` argument is held to 24,576 UTF-16 units of
  the command line, measured as it goes there: TOML-escaped and quoted as
  Windows quotes it (`CODEX_ROLE_MAX`, `command_line_units`), the rest of the
  32,767 left to the executable and the other arguments. Over it a helper's turn
  is refused, a chat starts without its layers (`role.layers-off`). The
  overview marks every entry given per session that would, for some role on
  some Codex account, with Luna's text for the role and the account's own
  value, not fit (`codex_overflows`, shown as the entry's problem). Codex sends
  the value at a thread's start and after a compaction; `exec resume` and
  `codex resume` rebuild the context from the rollout, so a changed value
  reaches a running thread only then.
- The system texts are read before the background lock (`background/run.rs`
  `spawn_turn`, `role_texts`): reading an override waits on the system lock
  while an adoption swaps one in.
- What was given is applied as `role.layers` (whose texts, the entries) and
  `constraints` (their words).
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
  Code would otherwise list it scoped to Luna's folder. Such a copy is compared
  and written as the renamed skill.
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
  account's `CLAUDE.md` in `claudeMdExcludes`, the other file first in the
  session's [role layers](#role-layers), written at every start to
  `hooks/<id>/CLAUDE.md`: memory is read again on resume, unlike
  `--append-system-prompt-file`. Codex reads its account's
  `AGENTS.override.md` or `AGENTS.md` with no way to turn it off, so Codex
  columns are disabled.

## The pass

The library is the source of truth (ADR-0030); each destination holds a copy.
A pass compares three things per destination: the library's files, what is
there, and the ledger's hashes of what Luna wrote last
(`<data>/library-state.json`). The ledger alone makes a destination Luna's: a
copy Luna wrote, adopted (byte for byte the library's when first seen) or took
in (an account's own instructions made the library's, `library_take_in`).

In a skill's folder only Luna's part is looked at (`luna_part`): the files at
the paths the ledger notes for it and those the library has for it, matched
without case on Windows. Anything else in the folder (a `__pycache__/`, a
`node_modules/`, a `.DS_Store`, a `*.luna-tmp` someone named so) is never
compared, backed up, written over or removed: `write_over` writes and removes
only within that part, and `take_out` removes Luna's files, the folders that
leaves empty, and the skill's folder only if nothing is left in it. A link or
junction inside the folder is read past (`read_dir_files`, said in the log
once a run): never followed, written over or removed, so it holds up neither
the copy's updates nor its removal. Adoption alone looks at the whole folder:
only one holding the library's files and nothing else becomes Luna's.

| Luna's part on disk vs. library and ledger | Status looking | Acting |
| --- | --- | --- |
| equal to the library | `current` | adopted where the ledger has no note (the whole folder equal) |
| missing (deleted there, or not yet written) | `missing` | written |
| equal to the ledger, behind the library | `behind` | written |
| different from the ledger (changed there) | `behind` | backed up, then written over |
| no ledger entry and different | `taken` | nothing: someone else's |
| a place 0.19 kept (`disowned`) holding anything | `taken` | nothing, even equal to the library |
| no longer wanted, equal to the ledger | not shown | taken out |
| no longer wanted, different from the ledger | not shown | backed up, then taken out |
| no longer wanted, changed, in a listed folder | not shown | its note goes, the file stays |
| in an account not listed now, or a folder no longer listed | not shown | nothing; the note stays |
| a place left out (`library.skipped`) | `skipped` | nothing: `library_skip` dropped its note, so whatever is there is someone else's |

`taken` clears on its own once the file is gone (the next pass writes the
library's copy there) or once the place is left out (`skipped`); nothing Luna
does writes over it. The old window lists it under its Needs a look, where
only **Leave out** still works.

What a change there had made goes to `<data>/library-backups/` (swept after
30 days) before the library's goes over it or it is taken out, each backup in
a folder of its own: `backup_slot` makes it exclusively, with a count after a
name taken, so two accounts' `skills/review` backed up in one millisecond do
not share one. A backup that will not write leaves the copy as it is, `failed`
for the pass with why, and the next pass tries again. `library-backups` that
is a link or junction, or no folder, takes no backup (`backups_are_lunas`),
and the sweep then takes nothing out; otherwise it takes out only folders of
`backup_slot`'s naming (`YYYYmmdd-HHMMSS-mmm-<tail>`), never one that is a
link, and `remove_dir_all` follows no link below. Each write over a changed
copy, each copy put back where it was deleted and each changed copy taken out
is logged with its path and its backup. Nothing is backed up of a copy still
as Luna wrote it.

After the backup, the pass writes or takes out a copy file by file, and looks
at each file again right before its own write, rename or removal
(`write_over`, `take_out`, `file_still`): one no longer as read and backed up
(a CLI or an editor saving it meanwhile, or a file made where there was none)
stops the copy, every file already done is put back as it was, and the copy
is left for the next pass, logged, its note as it was. A step that fails (a
rename Windows refuses while the file is held open without sharing) is
undone the same way: the place is `failed` for that pass, logged, never left
half the library's. Only where the undoing fails too does the ledger note
what is now exactly the library's, the rest keeping the note it had
(`noted_after_failure`). A start's pass has no window to show a `failed`
place, so `lay_out` logs each one with its reason, once a run while the reason
stays the same.

A copy no longer wanted, changed since an earlier Luna laid it into a listed
folder itself, is that folder's own wherever the folder lies, inside an
account or Luna's data folder too: its note goes, the file stays, said in the
log. A note of a place in no account `list_accounts` gives now, nor a listed
folder, nor Luna's `library-folders/` (an account taken off Luna's list with
its folder left, or one not there for the moment, or a folder no longer
listed) is kept as it is, logged once a run, and the copy is Luna's again
once the account is back.

Residuals, accepted:
- Ownership goes by the path: an account registered at a path is the one Luna
  manages there, so a different account later put at the same path takes over
  its notes.
- A file a person adds to a skill's folder byte for byte what the library
  later adds to that skill is taken for Luna's (it is the library's already),
  and goes without a backup once the skill is no longer wanted there.
- A `plugin.json` someone else put in Luna's own `luna` marketplace folder,
  for a slug of a folder the library lists and gives skills, is written over
  and noted as Luna's.
- Inside Luna's own data folder, foreign content under `library-blobs/`, or
  a folder in `library-backups/` named as Luna names its backups, is taken
  for Luna's and goes with them.
- A crash between the ledger's first save and the removal of
  `library-blobs/` leaves the blobs there for good: a leftover, read by
  nothing.
- The look again before each file narrows a save racing the pass to the
  instant between that look and the rename, not to none.

The Codex plugin manifest of a listed folder (`codex_manifests`) is written
while the plugin holds a copy and noted in the ledger's `manifests`; one is
taken out only where noted, so a plugin someone else put in Luna's
marketplace folder keeps its manifest. Neither is done where a folder on the
way down from the account's folder (`plugins`, `plugins/cache`, the
marketplace, the plugin, `.codex-plugin`) or the manifest itself is a link or
junction (`no_link_between`); the note stays for a later pass. A Claude Code
account's folder copies in `library-folders/<slug>/<name>/` are its own: while
that account is not listed they keep their notes and stay, as its account
copies do.

Deleting a skill from the library (`library_delete_skill`) backs up all of
its folder, temp-named files included, and is refused, nothing touched, where
the library's `skills` or the skill's folder is a link or junction, so a
library linked to an account's skills never deletes the account's own.

Luna 0.19 kept the bytes of every copy in `library-blobs/` for its window's
differences, and kept a copy no longer wanted but changed there in the
ledger's `kept` until the window settled it. A ledger read with no `format`
is 0.19's (`Ledger::older`), and its first save takes `library-blobs/` out
(`drop_old_blobs`: a link there goes as itself), once; a data folder with no
such ledger keeps any `library-blobs` it has. `kept` is moved into
`disowned` at the load and named in the log at that save: those copies stay
where they are, the person's own, never adopted, until nothing is there.
0.19's `worded`, the line it wrote into each chats' copy for taking one back
into the library, is not read.

Passes run when the window starts, before every session start, after every
change in the window and on Sync all, one at a time under a lock file beside the
ledger shared by window and core. Whatever writes takes it or is refused
(`lock`, `Nothing laid out: no lock on ...`), a settings change before it writes
the settings; only the overview reads without it (`lock_to_read`, said once a
run), the ledger being written whole. Where `lay_out` runs in a start:

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
- An account's rename or deletion holds the pass lock throughout: no pass sees
  the folder moved with the ledger still on the old path.
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
laid into a folder the library lists outside every account's and Luna's
data folder, that folder, whether or not it is given anything now: a listed
folder inside one of those is never the top, or its own link would go
unlooked at; one in a folder no longer listed has none and stays). The pass's writes and
  its removals (a copy no longer wanted, as Luna wrote it or changed there:
  the ledger names the place, not where a link on the way leads) look at
  every folder below that one on the way down (`no_link_between`, on
  `boundary::walk_down`) and refuse, naming the link, before anything is
  backed up, written or removed; the account folder itself may be the
  person's link. A removal refused keeps its ledger note for a later pass, and
  is logged once a run.
- Only a destination the ledger names is written over or taken out; one
  holding anything else is `taken` and left as it is.
- `system/` is read only for what Luna serves (`library/system.rs`): no pass
  lays anything of it out, and a skill named `session-handoff` under
  `skills/` is not the handoff's override.
- A system skill's name is reserved (`system.rs` `reserved`): a library skill
  of that name is `problem`ed in the overview, wanted nowhere (`units` skips
  it, so a copy an earlier Luna laid out goes as one no longer wanted) and hidden from no session (`hidden_here` skips it), so its
  name reaches no account, folder or start. `library_new_skill` refuses the
  name. Laid out, it would tell every session of the handoff.
- The window's System texts section reaches only `system/<name>` for a name
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
  - **Adoption** holds the system lock exclusive, none to be had refusing it
    before anything moves. It renames the skill into `system/` under a name
    nothing reads (the step that fails where the folder is held open), then an
    override there aside and the staged one into place, each undone if the
    next fails; the one set aside goes to the backups after. `resolve_in` holds the same lock (`<data>/library-system.lock`,
    a file lock: the window adopts, the core serves) shared while it reads, so
    no handoff is served Luna's text in the moment between the renames; without
    it, it reads all the same, said in the log, as a handoff is not to go
    without instructions.

## Failure modes

| Failure | Behavior |
| --- | --- |
| A file held open mid-save | that unit is `failed` for the pass; copies stay |
| A copy of Luna's edited in place (by hand or a model) | backed up, then the library's written over it at the next acting pass, logged; the edit is in `library-backups/` for 30 days |
| A copy of Luna's edited in place, then no longer wanted there | backed up, then taken out, logged |
| A file Luna did not write where a copy goes | `taken`, left alone; the library's copy goes there once it is gone |
| The backups will not write | the changed copy is neither written over nor taken out: `failed` for the pass, tried again at the next |
| A copy saved again while the pass is at it | left for the next pass, logged; nothing of it lost |
| `library-backups` is a link or junction | no backup written, so no changed copy written over or taken out; the sweep takes nothing |
| Files added inside a skill's folder of Luna's | not part of the copy: never compared, backed up or removed |
| Settings unreadable | no pass (`get_checked`) |
| The lock file cannot be opened or locked (tried three times, 50 ms apart; a disk that cannot lock files) | every change, Sync all, a take-in and an adoption refused with `Nothing laid out: no lock on ...`, nothing written; a session or a helper's turn starts without its pass, said in the log; the overview still reads |
| Sessions started outside Luna | see the account's copies only, not folder skills |
