# Auto-handoff

A person's chat with Luna tools may have auto-handoff on: past a threshold of
its context it is asked to write a handoff, and on its word Luna clears the
conversation in place and opens the next one from the file and the facts Luna
knows. User-facing behavior: [chats](../user-guide/chats.md#auto-handoff).
Decisions: [ADR-0020](../adr/0020-session-self-handoff.md) (the switch in
place) and [ADR-0022](../adr/0022-content-luna-serves-at-its-own-trigger.md)
(the instructions, served to that chat alone).

## Key modules

| Path | What it holds |
| --- | --- |
| `agents/handoff/mod.rs` | `luna_session_handoff` (`instructions`, `handoff`), who is offered it (`offers_tool`), the beat (`thresholds`, `switches`), the facts gathered for the prompt (`gather`), what the window is shown (`mark_of`, `asked_of`) |
| `agents/handoff/served.rs` | the instructions served, what is kept of them (`Served`), the panel's word (`applied_of`, `same_named`) |
| `agents/handoff/watch.rs` | when a notice is due, and the re-arm after a compaction |
| `agents/handoff/facts.rs` | what the notices, the tool's answer and the prompt say; the checklist (`SECTIONS`) and the look at a file for it (`missing_sections`) |
| `agents/handoff/file.rs` | the handoff file checked and copied |
| `agents/handoff/phase.rs` | the switch's machine |
| `agents/handoff/store.rs` | what is saved, `<data>/handoffs/handoffs.json` |
| `library/system.rs` | the instructions resolved: Luna's own or the library's override (`handoff_instructions`) |
| `resources/skills/session-handoff/SKILL.md` | Luna's own instructions, compiled in (`LUNA_HANDOFF`) |
| `src/features/chats/Handoff.tsx` | the panel's box, the divider, the chip's words (`askedWords`, `instructionsWords`) |
| `agents/handoff/moving.rs` | a chat moved to another account: its phases, the commands, the ask, the new chat's opening |
| `agents/handoff/document.rs`, `excerpts.rs`, `gitfacts.rs`, `scrub.rs` | the document Luna writes for a move, what it reads, and how it is cleaned |
| `src/features/chats/MoveChatDialog.tsx`, `moving.ts`, `MoveHint.tsx` | the move's dialog, the window's steps of it, the pane's offer |

## Who knows of it

`handoff` in a chat's registry settings, the threshold in percent, passed by
the window at every start (`ensure_*_session`) and set on a running chat by
`set_auto_handoff`. Only such a chat knows anything of auto-handoff:

- `tools/list` adds `luna_session_handoff` for it alone (`offers_tool`: tools,
  no parent, not an orphan, a helper or the dev caller) and notes that its CLI
  has it (`listed`, cleared by `register` at every start: a CLI lists tools as
  it starts, and Luna's server cannot announce a new one).
- A session it is not offered to that calls it all the same is refused as for a
  tool there is not, before its arguments are looked at (`mcp/dispatch.rs`):
  the refusal names no argument and no setting.
- The guide (`luna_guide.md`, `mcp/guide.rs` `TOPICS`) has no topic or section
  of it, and nothing of it is in `initialize`'s instructions or any other
  tool's description: every session with tools reads those.
- Its settings as another session reads them (`luna_session_get`, a listing;
  every row goes through `agents/list.rs`) lack `handoff` (`settings_shown`);
  the chat itself reads them whole.
- Nothing of it goes into a launch or an account but the chat's own: the
  instructions are no skill any CLI lists, and the library never lays `system/`
  out. A Claude Code chat with it on gets its folder of copies as an
  `--add-dir`, read without asking; a Codex one, under a sandbox that writes at
  all (`writes_files`), its drafts folder as an `--add-dir`, which Codex makes
  writable.
- A session that reads the chat's transcript (`luna_message_list` with
  `scope: "all"`) sees the notices and the prompt in it, as it sees all of it:
  the threat model's R1, accepted.

## The instructions

`luna_session_handoff` without `file` (`served::instructions`) answers as plain
text: the steps (the CLI's own jobs ended, the file written, the call with
`file`, the turn ended), where the file goes (a name of its own in
`<data>/handoffs/<chat>/drafts/`, `handoff-<random>.md`, not there yet, for the
CLI to make the file at, `new_draft`: a file Luna made first, private, Codex's
Windows sandbox could not write, and in the checked private folder nothing can
be put at a name nobody knows yet; or any file outside the data dir), what Luna adds to the next conversation itself (`LUNA_ADDS`, the one
list of it), and the instructions past their front matter, naming whose they
are and their SHA-256. A chat that can write no file (Codex's read-only
sandboxes, Claude Code's plan mode: `unwritable`) is told so.

`library::handoff_instructions` resolves them at each ask:
`<library>/system/session-handoff/SKILL.md` when it will do (`check_skill` with
the name `session-handoff`, UTF-8, at most 64 KB, something past the front
matter), else Luna's own with why the override was passed over. SKILL.md alone
is read: `system/` and its folder no link, the file opened as itself
(`boundary::open_no_follow`, no link followed, no wait on a FIFO), a regular
file of one name of this user's making by its handle (`secret::own_file`; the
library may be a folder of the person's that is not private, so not
`read_private`), read no further than the cap and a byte; the beat resolves
it for every firm notice, and nothing beside it is to hold that up. A library
skill named `session-handoff` under `skills/` is never read for it, and is laid
out nowhere (the name is reserved, [library](library.md#invariants)). The
library window's System skills section edits, compares and removes the override
([user guide](../user-guide/library.md#system-skills)). What a chat
was given (`Served`: `Given::Luna`, `Library`, or `Checklist` where the firm
notice's checklist was all it had, each with its hash, Luna's version or the
override's path) is kept on the chat's record (`Record::served`: the tool sets
it, a firm notice carrying the checklist sets it where nothing was, `told_of`),
copied into the switch the next call with `file` schedules
(`Switch::instructions`) and into its `Past`, and let go once a handoff goes
through.

**Luna's folders.** `handoffs`, a chat's folder in it and its `drafts` are
checked top down (`boundary::private_chain`, `secret::private_dir` at each level: a
folder, no link or junction, this user's, private) before anything is made or
read below them: the record loaded or saved (`store.rs`), a copy written
(`copies_of`), the drafts folder named or given to a CLI (`drafts_of`). In a
data folder others share, a link planted on the way would otherwise take all
of it to a folder another user holds; `handoffs` is one of the data folder's
own names (`paths.rs` `OWN_NAMES`) for the shared-folder privacy pass. The
drafts folder is made private, its DACL put back to the private three (this
user, SYSTEM, Administrators) and anything added before going, at every start
of the chat's CLI, before that CLI runs (`drafts_at_start`, from
`pty/start.rs`: a first start, a resume, a restart, a quick resume for
auto-handoff turned on while the chat runs). A start that cannot do so (a DACL
that denies this user WRITE_DAC, a file at its name) gives its CLI no
`--add-dir` for it (Claude Code not the chat's folder either), and until a
start can, the call without `file` names the chat no drafts folder but a file
of its own outside Luna's folder, and no file is taken from there
(`drafts_refused`, file.rs `read_checked`); the log and What Luna applied's
auto-handoff item say why. After the start no later look
touches its DACL (`drafts_of`, `secret::own_dir`: a real folder, no link,
owned by this user): Codex 0.167.0's unelevated Windows sandbox grants itself
an inheritable Modify ACE on each `--add-dir` once it runs, which a private
DACL put back would take away. No one else can add an ACE meanwhile, as that
takes WRITE_DAC, which the private DACL gives only this user, SYSTEM and
Administrators; what this user's own processes add is theirs (R3). Luna does
not try to tell the sandbox's SID from another (`cap_sid`,
docs/cli-compatibility.md, is not to be trusted for it; an account folder
others can write is outside what Luna defends, threat model B5). The data
folder's privacy check takes a drafts folder's ACEs as they are, for the same
reason, whether it checks the data folder whole or only Luna's own things in
it, `handoffs` among them (`secret.rs` `handoff_drafts`); its resetting pass puts them back to inherit, Codex's grant with them until
its next start. What lies in them is checked at its own name: `handoffs.json` and
a copy are read only as private files of this user's, by their handle, never
through a link (`secret::read_private`; a store that is not one is not taken
up, and the log says so), a draft only as one of this user's making
(`secret::own_file`: owned by this user or what this process makes files as,
of one name, open to its sandbox's SID as it inherits the folder's grant), and
a save renames a new private file into place, over a link rather than through
it. A chat deleted has its folder removed only where neither it nor
`handoffs` is a link.

## The threshold

The `luna-handoff` thread, every 2 s: for each opted-in chat whose CLI has
listed the tool, at a turn count it has not looked at with the turn over,
`Context::of` its transcript.

- Past the threshold one line in `Registry::news` (the notifier types it): go
  to `luna_session_handoff` without arguments for how. Past `firm_at`
  (threshold + 10, kept within 85-95) a firmer one; with Luna's own
  instructions in effect it carries their six headings inline (`SECTIONS`),
  with an override it sends the chat to the call. Once each per conversation
  (`Told`, by the transcript's name), each ask's percentage kept
  (`Told::asked`).
- A conversation a handoff began (`Record::fresh`) keeps its first reading as a
  floor and is asked only past it by `GROWTH` (5 points): one that begins above
  the threshold is not sent straight into the next handoff.
- **Re-arm.** A conversation told of its threshold whose context is then well
  under it (`rearm_below`: 15 points, or half the threshold where that is less)
  starts over, level and floor, and is asked again once past the threshold.
  Its tokens say why it fell: fewer than at its last notice by a tenth and
  more (`Told::tokens`, `compacted`) is a compaction, counted
  (`Told::compacted`) and shown; as many or more is a larger window (a model
  with more context), logged as the context falling and not counted. Both CLIs
  keep the conversation's transcript through their own compaction (seen in
  their code, [CLI compatibility](../cli-compatibility.md)); one that started a
  new transcript would read as a new conversation, asked again all the same,
  the earlier asks then dropped from the chip.
- A switch scheduled drops a notice not yet typed.

## The call with a file

- **The copy.** `file.rs`: absolute, `.md` / `.markdown` / `.txt`, opened with no
  link followed (and, off Windows, without waiting, so a FIFO swapped in does not
  hang the open) and checked on its handle, at most 128 KB of UTF-8 text (a
  leading byte order mark kept in the copy, skipped by the empty and heading
  checks: `without_bom`), refused
  inside the data dir after `canonicalize` but right in the calling chat's own
  drafts folder, that folder resolving where its names say (`own_draft`); all of it on a thread of its own
  given 10 s (`READ_WAIT`), outside the records' lock; a reader given up on keeps
  its place until its thread ends (`Readers`: one per chat, four in all, a call
  past either refused): the number is reserved
  first (`Record::reserved`, never saved, one call at a time per chat) and taken
  past any copy already on disk (`next_free`), so one a stopped core left is never
  written into. The copy is made new and private as `<data>/handoffs/<chat>/<n>.md`
  and flushed before the switch is recorded, its SHA-256 on record.
- **A thin file.** Written to Luna's own instructions (served them, or served
  nothing while they are in effect, `checks_sections`), the headings of
  `SECTIONS` the file lacks are named in the answer (`missing`, by name, case
  aside, a `None.` section present); nothing is refused. An override's
  structure is not looked for.
- **Recording it.** The switch is recorded on disk (`write_private_synced`; a
  save that fails is undone and refuses the call), only while the reservation
  is still the call's and auto-handoff is still on, both looked at under the
  record's lock (turning it off takes that lock, clears the reservation and
  changes the registry inside it), under the notifier's
  exclusion for the chat (`hand_off`): whatever is going into the chat then (a
  notice, `tell_of`; a held send or a send, `going_into`) goes in first, into
  the conversation that writes the handoff, and from then on each of them, which
  looks again at `holds_text` under the same exclusion, goes nowhere until the
  switch is over. It is refused while the CLI's own jobs run
  (`activity::own_jobs`: what its turns started that is still alive, its
  subagents), and while one is under way.

## The switch

`phase.rs`. Every phase is taken by `Phases::advance`, for that handoff's
number, from the phase it was read in, on disk before any text goes in; one not
taken (given up meanwhile, a save that fails) types nothing. Turning
auto-handoff off gives up only a switch still `Scheduled`, under the same lock,
so it never strands a reset.

`Scheduled` (from the call: `notice_waits` holds the notifier, `deliver_held`
and `send` hold text for the chat) → the chat at rest on two looks
(`ready_for_notice`), no draft, no job of its own → a thread claims the terminal
(`TypingInto`) for the rest, once the copy holds (`copy_holds`: right in the
chat's checked folder of copies, a private file of this user's opened as
itself, its SHA-256 the one on record; otherwise given up, nothing typed, and
no prompt pointing at it, the chat and the person told why; the same for a
switch a core takes up again). Each text goes in by `Term::type_line`: a person's
keys held (`hold_keys`), the composer looked at again (anything in it, a stash,
keys within 5 s: not now), the phase taken, then the paste and the Enter. →
`Resetting`: `/clear` or `/new` typed → the CLI on another conversation: the
registry's `sessionId` (Claude Code, `claude::session::session_id`) or the
title's thread (Codex, `codex_thread`) → `Prompting`: the prompt made then, past
the reset (`run` takes it as a closure), and typed: `[Luna handoff N, not typed
by the user]`, the copy's path, that approvals the handoff quotes hold as far
as the user's quoted words go and no further, and `fact_block` (from
`list::list`, the debts, `held`, `undelivered_for`, `Registry::usage`, the
settings; per helper its model and effort, task, worktree, branch and SHA and
transcript; names and tasks fenced in double quotes as data); past 6,000
characters the facts go in `<n>-facts.md` beside it → the transcript other than
the one left holds that opening → done. Then what waited goes in. A person
writing in the new conversation before the prompt holds it back (`Recovering`)
until they are through.

## Failures and restarts

- 60 s without a new conversation, 60 s without the prompt in the new
  transcript, 30 min without rest, 10 min with its CLI not running, the CLI's
  own jobs at rest, or a phase that will not save: given up, kept on record
  (`Past`), the chat told by a line in `news` (before the reset: it goes on as
  it was, and may call again only if the tool is still its; after: what it is,
  the prompt again, its facts as of then) and the desktop by a notification. A
  draft holds it as long as it stands, told to the desktop after a minute.
- A core restart: `from_bytes` makes a switch in `Resetting` or `Prompting`
  `Recovering`. Once the chat is back and at rest: on the conversation it left,
  `Scheduled` again (two resets at most); on another, done if its transcript
  holds the prompt, else the prompt is typed.

## The window

- `session_meta` carries the latest `HandoffMark` (under way, done, failed) for
  the pane's divider, and `HandoffAsked` (the asks of the conversation and its
  compactions since, while no handoff is under way or through) for the pane's
  `handoff N%` chip, which then reads `· asked` and says when in its tooltip.
- `applied_by_luna` carries `HandoffApplied` for a chat with it on: the
  instructions it would be served now (`SystemSkillState`: Luna's with its
  version and hash, the override with its path, or the override broken and
  why) and the `session-handoff` skills its CLI may list from its folder
  (`.claude/skills` for Claude Code, `.agents/skills` for Codex) or account
  (`skills/`), which the panel warns of.
- `handoff_file` and `handoff_transcript` read the copy and the conversation
  left by the record's paths (`background::view_of`). A conversation's first
  prompt that is Luna's handoff prompt is no name for the chat (`pty/probe.rs`
  `pin_name`).
- `forget` drops a deleted chat's record and its folder of copies; a chat keeps
  its last 20 on record.

## Moving a chat to another account

[ADR-0023](../adr/0023-moving-a-chat-to-another-account.md). The chat's
conversation stays; a new chat on another account of the same CLI goes on from
a document the person confirmed. User-facing behavior:
[chats](../user-guide/chats.md#moving-a-chat-to-another-account).

**Phases** (`moving.rs`, `Record::moving` of the old chat's record, each on
disk before what follows): `Asking` → `Asked` (the ask route only) → `Shown`
→ `Confirmed` → `OldStopped` (where the old chat stops) → `RowCreated` →
`Prompting` → `Done`; `Failed` and `Canceled` end one. A step taken again
answers as it stands. A core taking the record up gives up a move that is
`Shown`, `Asking` or `Asked` (nothing unconfirmed is ever sent), and takes
`Prompting` back to `RowCreated`, which types the line only if the new
transcript lacks it (`taken_up`).

**Commands.** `move_prepare` builds Luna's document (bounded, within 30 s) and
keeps the move as shown; `move_ask` asks the chat; `move_preview` reads what
it wrote back; `move_confirm` is Send; `move_step` is the window's word that it
stopped the old chat (`oldStopped`, looked at: its CLI no longer runs) or made
the row (`rowCreated`); `move_cancel`; `move_list`. The window's part is
`features/chats/moving.ts` (`carryOut`, and `useMoves`, which goes on with a
move a restart left half way).

**Send is bound** (`binding`) to the text's SHA-256, the account, the launch
settings the window sends and the line Luna types, made once with the preview
(`Move::line`). The text is refused when it names a folder of an account or of
Luna's in any spelling (`scrub::named_roots`), holds characters no screen
shows, or is over 96 KB; it is written private as `handoffs/<new>/1.md`, and
the new chat's record says it was opened by a move (`moved_from`), which gives
its Claude Code starts that folder as an `--add-dir` (`moved_dir`,
`moved-chat`).

**Opening** (`beat`): the line is typed into the new chat only when it is
registered as confirmed (`start_mismatch`: account, folder, permissions,
tools, an agent, Codex's unasked hooks), at a strict prompt (`strict_prompt`:
the composer drawn and empty, no turn, nobody typing), its copy still the one
confirmed. No wait shorter than a day while its CLI runs: an account's
first-run screens take a person. Done once its transcript holds
`[Luna move <id>, not typed by the user]`; 60 s after typing without it,
given up and the person told.

**Worktrees.** A Claude Code chat whose start had `worktree` moves into the
same worktree by name, its old chat stopped first; `opening_refusal` refuses
the new chat's start while the old one runs, and, once the new one is in, the
old one's while the new one runs (the session that made a worktree removes it
on exit when clean, whoever is inside: docs/cli-compatibility.md). Deleting
either chat keeps a worktree the other's row names. A Codex chat whose folder
is in its account's folder does not move (`codex_worktree_refusal`).

**Luna's document** (`document.rs`): rules of going on; the person's note;
the state (folder, git as of a time: `git status`, `git diff --stat`, commit
subjects since the chat began; how it stopped, from `activity::limit_of` and
its turn; the paths its edit tools were called on, as attempts); the task (the
last handoff that went through, its hash checked, else the first prompt and
the CLI's last compaction summary); the last plan proposed; the conversation
by turns, newest first within about 40 KB (the last request and answer to
16 KB, earlier answers to 3 KB, requests to 6 KB, start and end kept, an
unfinished turn's narration to 400 characters); helpers, which stay; first
steps. Every excerpt is fenced by a random word per document, a line opening
`[Luna ` in one escaped. The transcript is read 512 KB from its start and 16 MB
from its end, lines over 1 MB skipped and counted, its size and time looked at
before and after (`excerpts.rs`); git runs with five seconds and 256 KB a
command, without the repository's file monitor, optional locks, external diff
or text conversion (`gitfacts.rs`). The plan, the summary and the edit paths
come from shapes no CLI documents and are left out silently where they differ.

**Cleaning** (`scrub.rs`): roots (Luna's data folder, the accounts' root, each
CLI's folder of accounts and each account, `~/.claude` and `~/.codex` as paths
and as written from the home folder) matched with either slash, JSON's doubled
backslash, Git Bash's `/e/` and WSL's `/mnt/e/`, case aside on Windows, and cut
with the rest of their path; token shapes (known prefixes, PEM private keys,
a URL's `user:password@`, `NAME=value` where the name says secret) taken out;
zero-width, bidirectional and tag characters taken out; long high-entropy
strings listed, never taken out.

**The ask** (a chat offered `luna_session_handoff` whose CLI has listed it,
not in plan mode or a read-only sandbox, running, not stopped on a limit):
`Asking` until the chat stands at a strict prompt, then the fixed line
`[Luna move <id> request, not typed by the user]` typed under `Asked`. The
tool without `file` then serves the move's own text (`facts::move_how_to`,
a drafts name `move-<id>-<random>.md`), never the handoff's instructions; with
`file` it is collected (`asked_file`): copied and checked as a handoff's file
is, Luna's facts added, kept private as `move-<id>.md` in the chat's folder of
copies, nothing scheduled or reset, the CLI's own jobs no matter. A file named
`move-…` when no move is asking is refused, so a late answer never becomes a
handoff. No threshold notice goes in while it is asked. Ten minutes to come to
rest and ten more to answer; a question of its CLI's (a permission to write)
holds it without failing it.
