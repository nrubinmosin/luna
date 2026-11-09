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
`file`, the turn ended), where the file goes (a file of its own in
`<data>/handoffs/<chat>/drafts/`, `handoff-<random>.md`, made new, empty and
private for the answer, `new_draft`, so no link at a name used before is handed
over; or any file outside the data dir), what Luna adds to the next conversation itself (`LUNA_ADDS`, the one
list of it), and the instructions past their front matter, naming whose they
are and their SHA-256. A chat that can write no file (Codex's read-only
sandboxes, Claude Code's plan mode: `unwritable`) is told so.

`library::handoff_instructions` resolves them at each ask:
`<library>/system/session-handoff/SKILL.md` when it will do (`check_skill` with
the name `session-handoff`, UTF-8, at most 64 KB, something past the front
matter), else Luna's own with why the override was passed over. SKILL.md alone
is read: `system/` and its folder no link, the file opened as itself
(`boundary::open_no_follow`, no link followed, no wait on a FIFO), a regular
file by its handle, read no further than the cap and a byte; the beat resolves
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
checked top down (`private_chain`, `secret::private_dir` at each level: a
folder, no link or junction, this user's, private) before anything is made or
read below them: the record loaded or saved (`store.rs`), a copy written
(`copies_of`), the drafts folder named or given to a CLI (`drafts_of`). In a
data folder others share, a link planted on the way would otherwise take all
of it to a folder another user holds; `handoffs` is one of the data folder's
own names (`paths.rs` `OWN_NAMES`) for the shared-folder privacy pass. What
lies in them is checked at its own name: `handoffs.json` and a copy are read
only as private files of this user's, by their handle, never through a link
(`secret::read_private`; a store that is not one is not taken up, and the log
says so), a draft only as one of this user's making (`secret::own_file`), and
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
  hang the open) and checked on its handle, at most 128 KB of UTF-8 text, refused
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
