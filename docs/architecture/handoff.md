# Handoff

A person's chat with Luna tools may be asked to write a handoff: by the person
(the pane's Hand off now), by auto-handoff past a threshold of its context, or
by a move. On its word Luna clears the conversation in place and opens the
next one from the file and the facts Luna knows. User-facing behavior:
[chats](../user-guide/chats.md#handoff). Decisions:
[ADR-0020](../adr/0020-session-self-handoff.md) (the switch in place),
[ADR-0022](../adr/0022-content-luna-serves-at-its-own-trigger.md) (the
instructions, served to that chat alone) and
[ADR-0024](../adr/0024-handoff-on-request.md) (listed to every chat with Luna
tools, answered only when asked).

## Key modules

| Path | What it holds |
| --- | --- |
| `agents/handoff/mod.rs` | `luna_session_handoff` (`instructions`, `handoff`), who is listed it (`may_list`), the asks (`open_ask`, `asked_now`, `ask_holds`, `handoff_now`, `manual_ask_of`, `drifted`), the beat (`thresholds`: `to_look`, `look_at`, the look now `checks_now`; `switches`), the facts gathered for the prompt (`gather`), what the window is shown (`mark_of`, `asked_of`) |
| `agents/handoff/served.rs` | the instructions served, what is kept of them (`Served`), the panel's word (`applied_of`, `same_named`) |
| `agents/handoff/watch.rs` | when a notice is due, and the re-arm after a compaction |
| `agents/handoff/facts.rs` | what the notices, the tool's answer and the prompt say; the checklist (`SECTIONS`) and the look at a file for it (`missing_sections`) |
| `agents/handoff/file.rs` | the handoff file checked and copied |
| `agents/handoff/phase.rs` | the switch's machine |
| `agents/handoff/store.rs` | what is saved, `<data>/handoffs/handoffs.json`; the ask (`Asked`, `AskKind`) and the re-arm of a record from before asks were kept (`rearmed_unasked`) |
| `library/system.rs` | the instructions resolved: Luna's own or the library's override (`handoff_instructions`) |
| `resources/skills/session-handoff/SKILL.md` | Luna's own instructions, compiled in (`LUNA_HANDOFF`) |
| `src/features/chats/Handoff.tsx` | the panel's box, the divider, the chip's words (`askedWords`, `instructionsWords`) |
| `agents/handoff/moving.rs` | a chat moved to another account: its phases, the commands, the ask and how it ended (`AskEnd`), what blocks one (`ask_blocked`), the new chat's opening |
| `activity/limit.rs` | whether a usage limit still holds a chat (`limit_in_force`, `LimitFact`), by the readings `claude/limits.rs` and `codex/limits.rs` give |
| `agents/handoff/document.rs`, `excerpts.rs`, `gitfacts.rs`, `scrub.rs` | the document Luna writes for a move, what it reads (the skills a conversation loaded too, `skills_in`, for the handoff's facts as well), and how it is cleaned |
| `src/features/chats/MoveChatDialog.tsx`, `moving.ts`, `MoveHint.tsx` | the move's dialog, the window's steps of it, the pane's offer |
| `src/features/chats/handoffNow.ts`, `features/panes/Pane.tsx` | the pane's Hand off now: when it is offered (`handoffNowRefusal`), the core's refusal shown over the pane, the quick resume |

## Who knows of it

Every person's chat with Luna tools (`may_list`: tools, no parent, not an
orphan, a helper or the dev caller), auto-handoff on or not. `handoff` in a
chat's registry settings is only the threshold in percent, passed by the
window at every start (`ensure_*_session`) and set on a running chat by
`set_auto_handoff`. Only such a chat knows anything of the handoff:

- `tools/list` adds `luna_session_handoff` for it alone and notes that its CLI
  has it (`listed`, cleared by `register` at every start through `unlisted`: a
  CLI lists tools as it starts, and Luna's server cannot announce a new one).
  A chat whose CLI has not listed it is asked nothing until it starts again:
  Hand off now answers `resume` for the window's quick resume.
- Listed is not answered: see [The ask](#the-ask).
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
  out. A Claude Code chat with Luna tools gets its folder of copies as an
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
a CLI started before it was listed the tool). A start that cannot do so (a DACL
that denies this user WRITE_DAC, a file at its name) gives its CLI no
`--add-dir` for it (Claude Code not the chat's folder either), and until a
start can, the call without `file` names the chat no drafts folder but a file
of its own outside Luna's folder, and no file is taken from there
(`drafts_refused`, file.rs `read_checked`); the log and What Luna applied's
`handoff-tool` item say why. After the start no later look
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

The `luna-handoff` thread, every 2 s: for each chat with a threshold whose CLI has
listed the tool, at a turn count it has not looked at with the turn over,
`Context::of` its transcript, only where that transcript is positively the
conversation the CLI is on (`transcript_is`: its name's own id, Claude Code's
`<session id>.jsonl` or Codex's `rollout-<time>-<thread id>.jsonl`, is the
live one's). After Codex's `/new` the rollout is the old thread's until the
new one writes its own (`rollout_on`), and a transcript not found now is the
last found: neither says how far the conversation the CLI is on has come, so
that beat is skipped and the turn looked at again at the next.

- **Turned on.** `set_auto_handoff` with a threshold, on or at another one,
  marks the chat (`checks_now`, numbered): the beat looks at it as soon as it
  is at rest, at whatever turn count, by the same look as a turn's end
  (`to_look`, `look_at`), so a chat already past the threshold is asked then
  rather than at its next turn's end. The mark waits while the CLI has not
  listed the tool (the window's quick resume, `resume`, brings it), a
  handoff or a move of the chat is under way, or the person's ask is open;
  a look lets go only of the mark it was taken under, so one set again
  meanwhile gets its own. In memory only: a core started again has looked
  at no turn yet (`Beat::turns`), so its first beat looks at every chat at
  rest all the same. Turned off, the mark goes, with the threshold's ask and
  its notice not yet typed (dropped under the record's lock, which the
  notice is queued under).

- Past the threshold one line in `Registry::news` (the notifier types it): go
  to `luna_session_handoff` without arguments for how. With it the threshold's
  ask (`AskKind::Auto`) is opened on the record, bound to the conversation
  whose context passed it, and only if, under the record's lock, auto-handoff
  is still on at that threshold and no person's ask, move or handoff
  took the chat while its context was read (`ask_auto`); the notice is queued
  under the same lock, so a move or the person's ask begun right after drops
  it as it opens. None is opened while a move of the chat is under way
  (`moving::under_way`), and the notifier types none then: one put together
  before the move began is dropped where it goes in (`withheld`). Past `firm_at`
  (threshold + 10, kept within 85-95) a firmer one; with Luna's own
  instructions in effect it carries their seven headings inline (`SECTIONS`),
  with an override it sends the chat to the call. Once each per conversation
  (`Told`, by the transcript's name), each ask's percentage kept
  (`Told::shares`, read under its old name `asked` too).
- A conversation a handoff began (`Record::fresh`) keeps its first reading as a
  floor and is asked only past it by `GROWTH` (5 points): one that begins above
  the threshold is not sent straight into the next handoff.
- **Re-arm.** A conversation told of its threshold whose context is then well
  under it (`rearm_below`: 15 points, or half the threshold where that is less)
  starts over, level and floor, and is asked again once past the threshold.
  The threshold's ask closes with it (`kept_told`), so it never stands on a
  conversation no longer near its limit.
  Its tokens say why it fell: fewer than at its last notice by a tenth and
  more (`Told::tokens`, `compacted`) is a compaction, counted
  (`Told::compacted`) and shown; as many or more is a larger window (a model
  with more context), logged as the context falling and not counted. Both CLIs
  keep the conversation's transcript through their own compaction (seen in
  their code, [CLI compatibility](../cli-compatibility.md)); one that started a
  new transcript would read as a new conversation, asked again all the same,
  the earlier asks then dropped from the chip.
- A switch scheduled drops a notice not yet typed.
- A record from before asks were kept, told of its threshold with no ask, is
  re-armed as it loads (`store.rs` `rearmed_unasked`): the threshold asks it
  again at its next look rather than leave it refused and never asked.
  So is a conversation whose threshold's ask gave way and nothing answered
  it: the person's ask that took its place lapsed or was withdrawn
  (`lapsed`), a move retired it as it began and then ended with the chat
  not stopped (`moving::threshold_retired`, `end_where`), or auto-handoff was
  turned on again with no ask open (`set_auto_handoff`, which looks at once,
  above); otherwise the
  threshold, noticing only above a level it told of, would leave a chat past
  it unasked until its firm mark.

## The ask

`luna_session_handoff` is listed to every person's chat with Luna tools, and
answers, with `file` or without, only while the chat is asked
(`Record::asked`, saved in `handoffs.json`, or a move's ask, moving.rs).
Unasked, it refuses naming the pane's menu (`NOT_ASKED`), and nothing is kept;
a call in the turn that answered a move's ask Luna has since closed hears that
the move is over instead (`moving::MOVE_OVER`), not that nobody asked it.

- **Who asks.** `AskKind::Auto`, the threshold's notice ([above](#the-threshold));
  `AskKind::Manual`, the person's Hand off now (`handoff_now`, a core
  command); or a move's ask (`MovePhase::Asking`, `Asked`). No tool opens one.
- **Hand off now** is refused, with what stands in the way (`now_refusal`):
  not a person's chat with Luna tools, a sandbox or mode that writes no file
  (`served::unwritable`), a handoff under way, a move of the chat under way
  (asking it, its preview under review or its steps after confirmation:
  `moving::under_way`), an ask of the person's open, the CLI's own jobs at work, its CLI not running. Otherwise
  the ask is opened in place of one the threshold left open, a threshold
  notice not yet typed is dropped, and the answer says whether the window is
  to resume the CLI on the same conversation first (`HandoffNow::resume`, for
  a CLI that has not listed the tool).
- **Typed strictly.** The person's request (`facts::manual_notice`) goes in
  once the CLI has listed the tool and stands at a strict prompt
  (`moving::prompt_verdict`), on a thread of its own with the terminal
  claimed, the turn state looked at again under the held keys, and taken as
  typed (`Asked::typed`, with the conversation it went into) only then
  (`type_manual`, `manual_typed`), as a move's ask is: not through the
  notifier, which could paste it into a Codex turn just begun. Where Luna
  cannot name the conversation then, it is not typed, and tried at a later
  beat. Until it is typed the person's ask authorizes nothing: the person may
  start another conversation while it waits, and text there must not find
  the tool open.
- **Bound to a conversation.** Each ask holds only in the conversation it
  asks (`Asked::conversation`, `ask_holds`), both known and the same; where
  either is not known the call is refused (`NOT_YET`, "call again in a few
  seconds"), not accepted. The threshold's ask is bound as it opens to the
  conversation whose context passed it, the person's as its request is typed
  to the one it went into. The beat closes an ask whose chat is found on
  another conversation the person started (`/clear`, `/new`: `drifted`), and
  leaves it be while Luna cannot name the one the chat is on. A Codex chat's
  conversation is the thread its title names, except one of its subagents'
  it is watching (`/subagents`): that leaves it on its own thread
  (`codex/session/rollouts.rs` `own_thread`), so watching a child closes no
  ask of the parent's. Luna's own switch, which changes the
  conversation on purpose, closes its ask as it ends. A call from another
  conversation, or from one Luna cannot name, schedules nothing.
- **Lapse.** The person's ask closes `MANUAL_ASK_MS` (10 minutes) after its
  request was typed, the chat told (`facts::manual_lapsed`) and the desktop
  notified, unless a handoff answering it was scheduled since the look
  (`lapsed`). One whose request Luna could never type (the chat never at its
  prompt, its conversation not known, a draft that stays, its CLI not
  running) is withdrawn `MANUAL_DELIVER_MS` (10 minutes) after it was opened,
  looked at before each try at typing it and across restarts (`Asked::at` is
  saved): that same ask only, never one typed since, with its own word to the
  chat (`facts::manual_undelivered`) and the desktop (`undelivered`). Turning auto-handoff off closes only the threshold's ask and a
  switch it only scheduled; the person's is theirs.
- **A move takes its place.** A move of the chat the person begins, by its
  ask or from records, withdraws their ask still open, typed or not, as long
  as no handoff answers it yet (`moving::taken`, under the record's lock):
  the move is their later word, and the handoff the ask would bring resets
  the chat in the middle of the move. Its request not yet typed is dropped
  with the threshold's notice, one being typed finds it gone
  (`manual_typed`), and the pane and the desktop say the move took its place
  (`moving::ask_withdrawn`); a call the chat makes for it then is refused as
  unasked. A move is refused to begin while a handoff of the chat is
  scheduled or its file is being copied. While a move is under way the
  person's ask is neither typed nor closed (`manual_ask_of`); Hand off now
  refuses to open one then.

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
  is still the call's, the chat is still listed the tool and its ask still
  holds in the conversation the call came from (`schedule`, `ask_holds`), and
  no move of the chat began meanwhile (`moving::under_way`: its reset would
  come in the middle of the move), all looked at under the record's lock (turning auto-handoff off under the
  threshold's ask takes that lock, clears the reservation and changes the
  registry inside it), under the notifier's
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
auto-handoff off gives up only a switch still `Scheduled` that the threshold's
ask scheduled, under the same lock, so it never strands a reset; one the
person asked for goes on. A switch through closes the ask it answered, as
does one given up past its reset (`close`, `finish`); one given up before its
reset leaves the ask open, so the chat may call again while it holds, but for
one whose chat went on in another conversation (below), whose ask is gone
with that conversation.

The reset clears only the conversation that called (`Before::conversation`).
A chat at rest found on another one, the person's own `/clear` or `/new`
while the switch waited, is given up (`Step::Left`, `Ended::Left`, `LEFT`):
nothing reset, nothing typed, the copy kept, the ask closed, and the
conversation the person went on in told what became of the call
(`facts::left_line`) as the desktop is. The conversation is read again with
the person's keys held, right before `/clear` or `/new` goes in
(`Term::conversation`), so one they sent after the look is caught there too;
one Luna cannot name then types nothing, and the switch waits for it (30
minutes at most).

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
title's thread (Codex, `codex_thread`), recorded as the one the reset opened
(`Switch::opened`, saved before anything goes into it): once, never written
over, and only where no key of the person's came since the reset was typed
(`Look::keys`, the pty's `typed_at_ms` read after the conversation and
against its value under the reset's held keys). The reset's typing waits
seconds for a turn `/clear` never starts, and a `/new` of the person's in
that time would otherwise be taken for the reset's; any key, or another
conversation than the one on record, gives the switch up as `Stray`, nothing
typed → `Prompting`, the conversation read again under the held keys and the
prompt typed only if it is still the one on record: the prompt made then, past
the reset (`run` takes it as a closure), and typed: `[Luna handoff N, not typed
by the user]`, the copy's path, to load the skills the handoff names under
"Skills to load first" before the first reply, that approvals the handoff
quotes hold as far as the user's quoted words go and no further, and
`fact_block` (from `list::list`, the debts, `held`, `undelivered_for`,
`Registry::usage`, the settings; per helper its model and effort, task,
worktree, branch and SHA and transcript; the skills the conversation left
loaded, read off its transcript, `excerpts::skills_in`: names only, each once
in order of first use, from its first 256 MB, or "not known" where it cannot
be read; names, tasks and skills fenced in double quotes as data); past 6,000
characters the facts go in `<n>-facts.md` beside it → the transcript other than
the one left holds that opening → done. Then what waited goes in. A person
writing in the new conversation before the prompt holds it back (`Recovering`)
until they are through.

## Failures and restarts

- 60 s without a new conversation, 60 s without the prompt in the new
  transcript, 30 min without rest, 10 min with its CLI not running, the CLI's
  own jobs at rest, or a phase that will not save: given up, kept on record
  (`Past`), the chat told by a line in `news` (before the reset: it goes on as
  it was, and may call again only while its ask still holds; after: what it is,
  the prompt again, its facts as of then) and the desktop by a notification. A
  draft holds it as long as it stands, told to the desktop after a minute.
  The prompt goes into a failure's line only with the chat on the
  conversation its reset opened (`phase::failed`): past the reset on any
  other, or on none Luna can name once the prompt was typed, the switch ends
  as `Ended::Stray` with the failure's why, and the line names only the copy.
- The notifier types a failure's line later than it is made. One that carries
  the prompt is bound to the conversation its reset opened (`Record::continued`,
  by the line's SHA-256): it goes in whole only while the chat is still on
  that one, looked at as the notice is put together, again where it goes
  in, and a last time once the person's keys are held, right before the
  paste (`as_told`; delivery.rs `type_text`'s `stands`). A `/clear` or `/new`
  of theirs sent just before the hold may not have reached the CLI yet, which
  still reads as the old conversation: so the line also waits on any key of
  theirs since the notifier's look, or within the quiet that look asked for
  (`TYPING_QUIET_MS`; notifier `deliver_notice`, by `typed_at_ms`, which
  stamps a key as it is queued for the CLI). Either way nothing is typed
  (`NO_LONGER_STANDS`), the line stays queued, and the next look sees the
  conversation their keys took the chat to. On any other conversation, or
  where Luna cannot tell, a short line (`facts::continued_short`) naming the
  copy takes its place.
- A core restart: `from_bytes` makes a switch in `Resetting` or `Prompting`
  `Recovering`. On another conversation than the one it left and not the one
  its reset opened (`Switch::opened`), or with none on record (the core gone
  between saving `Resetting` and the reset reaching the CLI, while the person
  opened a conversation of their own), it is given up at once with nothing
  typed, before any wait for rest (`Step::Stray`, `Ended::Stray`, `STRAY`): a
  turn of the person's there outlasting the wait would otherwise fail it with
  the prompt in its line. The copy is kept, the ask closed, the chat told
  (`facts::stray_line`) and the desktop notified. Otherwise, once the chat is
  back and at rest: on the conversation it left, `Scheduled` again (two
  resets at most); on the one its reset opened, done if its transcript holds
  the prompt, else the prompt is typed.

## The window

- `session_meta` carries the latest `HandoffMark` (under way, done, failed) for
  the pane's divider, `HandoffAsked` (the asks of the conversation and its
  compactions since, while no handoff is under way or through) for the pane's
  `handoff N%` chip, which then reads `· asked` and says when in its tooltip,
  and `HandoffAsk` (`handoff_ask_of`: the ask open, its kind, when it was
  opened and, for the person's, when its request was typed and when it
  lapses). The menu's Hand off now and the `handoff asked` chip go by it
  alone, with no clock of the window's: a reload, or the person's `/clear`
  closing the ask, shows what the core holds. With the chat's CLI not
  running `session_meta` says nothing, and the window asks `handoff_ask`
  (the same `handoff_ask_of`) instead, so an ask that lapsed meanwhile leaves
  the chip and the menu.
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

[ADR-0023](../adr/0023-moving-a-chat-to-another-account.md), amended by
[ADR-0024](../adr/0024-handoff-on-request.md). The chat's
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

**Commands.** `move_facts` is what the dialog looks at first: the usage
limit that still holds the chat (`activity::limit_in_force`, a `LimitFact`:
its kind, since when, the full window with its share and reset, the reading's
time and age) and what keeps the chat from being asked (`ask_blocked`: no
Luna tools, a CLI that has not listed the tool, a
sandbox or mode that writes no file, its CLI not running, stopped on a limit
in force, a question of its CLI's), each with what clears it. `move_prepare`
builds Luna's document (bounded, within 30 s) and keeps the move as shown;
`move_ask` asks the chat; `move_preview` reads what it wrote back;
`move_build_from_records` takes a move still asking, or whose ask failed, to
Luna's document: it closes the ask as Cancel does (`ask_closed`), so a
document the chat sends late is refused and never replaces the preview, and
builds into the same preview with the move's destination and note;
`move_confirm` is Send, refused while a handoff of the chat is scheduled
(`Record::switch`, looked at under the record's lock); `move_step` is the window's word that it
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
its turn; the paths its edit tools were called on, as attempts; the skills
its conversation loaded, read before the window from the transcript's start
and within it as the rest is, the ask's turns left out); the task (the
last handoff that went through, its hash checked, else the first prompt and
the CLI's last compaction summary); the last plan proposed; the conversation
by turns, newest first within about 40 KB (the last request and answer to
16 KB, earlier answers to 3 KB, requests to 6 KB, start and end kept, an
unfinished turn's narration to 400 characters); helpers, which stay; first
steps, the first of them to load the skills it lists where there are any (the
chat's own document: those it says to load first). The new chat's line says
to load them, those its account has, before its first reply. Every excerpt is fenced by a random word per document, a line opening
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

**Which document.** The window has the chat write it by default; it
preselects Luna's only while `limit_in_force` holds. A limit holds from the
stop (its source's time: Claude Code's `StopFailure` hook, the Codex rollout
line; else when Luna first saw it) until every window that counts for the
chat's model is known to have lifted, each limit (the windows scoped to one
model, or to none) by its own windows: read after the stop with none full, or
with every window it showed full past its reset; with no reading, no window
that counts, or no reset to go by, it holds. A limit read only before the
stop and not full, or one the reading lacks, is not known to have lifted: a
general window's reset does not lift a Spark chat whose own limit has had no
reading since. Each Codex limit stands by its own newest reading
(`codex::limits::known_limits`): Luna's last answer from the endpoint, the
whole picture as of its time, and after it each limit's newest rollout
snapshot (`latest_rate_limits_by_id`: every limit a tail holds,
`rate_limits_in`, of the newest 32 rollouts of two days and the chat's own
wherever it falls, kept by the core once read, however old: a Spark week
full days before its reset is not dropped for a newer general reading to
stand in for), since a
snapshot holds one limit alone: a Spark chat's newer snapshot takes nothing
of the general limit's, nor a general one a full Spark window. A
window scoped to one model (Claude Code's Opus week, a Codex model's own
limit) holds only a chat on that model, the model of the turn that stopped
(`agents::stopped_model`): Codex's latest `turn_context`, answered or not, so
a chat switched to Spark whose first Spark turn hit Spark's limit is judged
by Spark; Claude Code's failed request names no model (`<synthetic>`) and
its transcript records no `/model` switch, so the model that answered last
(`last_model`, past those lines), which follows a switch once the new model
has answered, else its settings' model, the one it was started on. So a
chat switched with `/model` whose first turn on the new model stops on a
limit before that model has answered anything is judged by the model before
the switch, the one that answered last. With the
model unknown, every window counts (`UsageReading::for_model`). No usage percentage picks a source.

**The ask** (a person's chat with Luna tools whose CLI has listed
`luna_session_handoff`, not in plan mode or a
read-only sandbox, running, not stopped on a limit: `ask_blocked`; no
auto-handoff needed): `Asking` until the chat stands at a strict prompt, then
the fixed line `[Luna move <id> request, not typed by the user]` typed under
`Asked` (`asked_at`, `asked_turns` and the conversation it went into,
`asked_conversation`, kept; not typed while Luna cannot name that
conversation). It answers only once typed and only in that conversation
(`ask_here`): before, or in one the person started since, the tool refuses
(`NOT_ASKED_HERE`), and collection looks again as it takes the document.
The chat found on another conversation once it is typed fails the ask at
once (`left_asked`, `LEFT_ASKED`, `AskEnd::Left`), Ask again offered, rather
than the dialog saying it writes until the wait runs out. Opening the ask
takes the chat from the threshold's ask, which closes, its notice not yet
typed dropped (`taken`, `threshold_retired`): while the move asks, any
file the chat sends is the move's (`asked_file`), never a handoff, so the
threshold's ask could only refuse; the person asked for the move. A move
built from records retires it the same way as its preview is kept
(`move_prepare`, `taken`): a handoff the threshold asked for would
reset the chat in the middle of the move. The threshold stays quiet for the
whole move, any move, its preview under review and the steps after
confirmation included (`moving::under_way`): it opens no ask, the notifier
types none of its notices (`withheld`), and the person's Hand off now is
refused. It is re-armed as the move ends with the old chat not stopped,
before its stop or in a move that keeps it running (`stop_old` false;
`end_where`, `store::rearmed_unasked`): the chat goes on where it is and is
asked again if it is still past it. The person's own ask open is
withdrawn the same way, the person told ([The ask](#the-ask)). Not while a
handoff is under way or its file is being copied. The tool without `file` then serves
the move's text (`served::move_instructions`, `facts::move_how_to`, a drafts
name `move-<id>-<random>.md`): the handoff's instructions in effect, the
person's override included, under the move's own rules (helpers stay behind,
prior approvals confer nothing, the person previews first, the size limit),
recorded and logged as the handoff's are; with
`file` it is collected (`asked_file`): copied and checked as a handoff's file
is, Luna's facts added, kept private as `move-<id>-<random>.md` in the chat's
folder of copies, nothing scheduled or reset, the CLI's own jobs no matter.
Each call writes a file of its own: of two overlapping in one ask, the one
that takes `Shown` keeps its preview, and the other removes only its own. A file named
`move-…` when no move is asking is refused, so a late answer never becomes a
handoff; so is any call, with `file` or without, in the turn that answered an
ask Luna closed (Use Luna's document now, Cancel, a failure) within an hour of its
close (`answers_closed_ask`, `late_answer`), with `MOVE_OVER`: the move was
decided otherwise, and the chat is not to take it for a failure of its own.
The log says an ask is typed as it is taken as typed, before the paste
returns, and again once its CLI took it. No threshold notice goes in while it is asked. Ten minutes to come to
rest and ten more to answer; a question of its CLI's (a permission to write)
holds it without failing it. A failure is taken only while the move is still
asking (`ask_ended`, `failed_where`), so it never fails a preview Luna built
meanwhile or a move already confirmed. It keeps why and how it ended
(`AskEnd`: `Limit`, `Exited`, `Timeout`), and the status says so
(`askFailed`) and whether asking again makes sense (`ask_again`,
`may_ask_again`: not after a limit or an exit; after a timeout, once nothing
blocks an ask). The window offers Build from records, Ask again where it may
go through, and Cancel; it never builds Luna's document unasked.

**The ask's own turns** are no part of Luna's document: the turn the ask
opened is the ask's only while no turn has ended since it was typed and it
began before the ask closed (`in_ask_turn`), so a turn the person began after
an ask timed out is the chat's own work. The excerpts leave every turn the
ask opened out, a window that starts inside the ask's turn included
(`excerpts.rs`), and a turn under way since the ask is not reported as
unfinished work.
