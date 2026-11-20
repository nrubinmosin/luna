# Agents: MCP tools, registry, notifier

Sessions that run sessions. A chat started with **Luna tools** gets Luna as an
MCP server and can spawn helpers, talk to them, answer what they ask and delete
them. User-facing behavior is in the [user guide](../user-guide/agents.md);
decisions in [ADR-0012](../adr/0012-agents-over-mcp-with-notices.md), and in
the 2026-11-20 amendment of [ADR-0030](../adr/0030-scope-of-0-20.md): Luna
ranks none of the CLIs' modes.

## Responsibility

- Serve the MCP tools on the hub and authorize every call.
- Keep lineage (who spawned whom), tokens and rights.
- Track what each caller is owed and deliver it: in a `wait`, a `read`, or a
  notice typed into the caller.
- Type into terminals safely around a person's draft.
- Remember enough across a core restart to tell callers the truth
  ([background helpers](background-helpers.md#outliving-the-core)).

## Key modules

| Path | What it holds |
| --- | --- |
| `hub.rs` | loopback HTTP: `/hook/<chat>/<secret>`, `/mcp` (bearer); a thread per request in bounded lanes (`admit.rs`) |
| `mcp/mod.rs` | JSON-RPC: `initialize` (with `instructions`), `tools/list`, `tools/call`; `mcp::Call` per call in flight |
| `mcp/tools.rs` | the sixteen `luna_<resource>_<verb>` tools (`REMOVED`: the old names, refused with the current list) |
| `mcp/args.rs`, `mcp/dispatch.rs`, `mcp/guide.rs` | strict arguments (`arguments_fit`); each call to the agent layer and its log line; the guide by topic (`TOPICS`) |
| `agents/registry.rs` | the `Registry` behind one lock (`reg()`): tokens (by SHA-256), `Known` sessions, lineage, spawns under way, debts, waits, receipts, names, groups, helpers taken over |
| `agents/owed.rs` | answers owed (`Owed`), `hand_over` and its undo |
| `agents/ask.rs` | what a session waits on (`ask_of`), screen recognition of dialogs |
| `agents/read.rs`, `agents/said.rs` | transcripts and rollouts read with a byte cursor; what a session said and how a result puts it (`reply_of`, `report_of`) |
| `agents/lost.rs` | what never reached a helper |
| `agents/admission.rs` | Luna's own admission of text and folders (`reaches_luna`, `command_reach`, `plain_text`) |
| `agents/delivery.rs` | `luna_session_send`, typing into a chat's terminal (`TypingInto`, `type_text`, the draft put aside) |
| `agents/held.rs` | text held for a person's draft (`hold`, `deliver_held`; past 16 MB held for one chat a send is refused, saying why) and text a core went in the middle of typing (`reconcile_held`) |
| `agents/handover.rs` | a helper the person opens as a chat, handed over for good (`take_over`) |
| `agents/reparent.rs` | the person's move of a helper, with its line, to another chat (`move_helper`, `move_targets`), the chats refused a write into it (`asked_to_move`), what a row says of whose it is (`ownership_of`) |
| `agents/answer.rs`, `wait.rs`, `list.rs` | `luna_session_answer`, `luna_session_wait`, `luna_session_list` / `luna_session_list` |
| `agents/teardown.rs` | kill, delete, the person's guarded delete (`delete_by_user`, `at_stake`), `reap` (autoDelete), `forget` |
| `agents/persist.rs` | ties across cores: a helper's in `helpers.json`, the person's chats' debts, held sends, spawns under way and untold lines in `chat-ties.json` |
| `agents/spawn/` | `luna_session_spawn`: `account.rs` (account lists), `background.rs`, `folder.rs` (where the caller works, who else is there), `settle.rs` (spawns that never completed) |
| `agents/notifier/` | the notifier loop; `lines.rs` what a notice says; `usage.rs` quota and context news |
| `agentlog.rs` | `<data>/agents.jsonl`: a line per spawn and per end |
| `luna_guide.md` | the manual `luna_guide_get` returns a section at a time, by a fixed topic per tool and `notices`, `restart`, `helpers` (`mcp/guide.rs` `TOPICS`), or with no topic its opening and the topics; every session with tools reads it |

## How a session gets Luna

- **Claude Code:** `--mcp-config <data>/mcp/<chat>.json` with
  `"timeout": 1900000` and header `Bearer ${LUNA_MCP_TOKEN}` (the token in the
  environment, never in the file), plus `--allowedTools mcp__luna`.
- **Codex:** `-c mcp_servers.luna.url=…`, `bearer_token_env_var=LUNA_MCP_TOKEN`,
  `tool_timeout_sec=1900`, `default_tools_approval_mode="approve"`: Luna's tools
  ask no permission in any mode; the checkbox is the permission.
- Tokens are per session and reissued on every spawn. The instruction travels
  in `initialize`; together about 2k tokens once.
- Every session has `LUNA_SESSION_ID`; a helper also `LUNA_PARENT_ID`. A
  session is told its role at memory level (ADR-0026, [library: role
  layers](library.md#role-layers)): a lead (spawned with `tools`) or a helper
  that its task comes from another agent, to answer it in the language and
  form the task asks for, and what it still owes of the rules written for a
  person's chat: to stop, before its turn ends, the background work it started
  that nothing needs any more, keep each caveat with its claim, send nothing
  outward unasked, and install nothing and change nothing machine-wide or in
  its CLI account's folder unless its task names that change; a lead also that
  it owns what its helpers report and passes its constraints on. Those are
  system texts the person may override (`role-lead`, `role-helper`). Claude
  Code reads them as `hooks/<id>/CLAUDE.md` through `--add-dir` at every turn;
  Codex in `-c developer_instructions`, after its config stack's own value.
  The library can keep the person's chat's rules from it altogether (an
  entry's audience, [library](library.md#audiences)). Beside the task, a
  one-line note (`note-lead`, `note-helper`, `pty::helper_prompt`) says it
  comes from another agent, not a person, after a Claude Code helper's first
  prompt and every Codex one's: a Codex thread may have compacted its task far
  from its developer message. Its tag is `[A note, not part of the task: …]`,
  naming neither Luna nor any session. The transcript readers drop the note
  (`without_note`): Luna's defaults, every override note ever served (by
  hash, `<data>/notes-served.json`, the last 64) and the lines earlier Lunas
  wrote under their tag. Slash-command prompts go without.
- **Constraints** (`SpawnParams::constraints`, `spawn::Constraint`): text a
  spawn adds, word for word, to the chain its caller carries (read from the
  caller's registry settings, `constraints_of`); checked as plain text (no
  control characters but line breaks and tabs), at most 4 KB per spawn and
  16 KB per chain. The chain goes on the helper's `Spec` and into its registry
  settings as `"constraints"` (kept across starts, `CORE_SETTINGS`; shown by
  `luna_session_get`), never changes after the spawn, and is the last of its
  role layers at every turn, under a heading that lets it only narrow the
  session's other instructions (the stricter applies where they conflict:
  an injected lead's text must not loosen its descendants' rules). Taken
  over as the person's chat, a session drops it with the other helper
  settings (`HELPER_SETTINGS`), so no helper it starts inherits it; a Codex
  thread still carries it (see "Handed over"). Text, not enforcement:
  what a session may do is still its mode's, its CLI's.
- The instruction says the CLIs' own subagents cannot use another provider or
  account (without it a Codex orchestrator used its own `spawn_agent`).
- A helper gets Luna's server and none of its account's, unless spawned with
  `connectors: true` (`Spec::connectors`, kept as `connectors` in its settings,
  one of `CORE_SETTINGS`). Claude Code:
  `--strict-mcp-config` (no user, project, plugin or claude.ai server) and a
  settings file (`HelperSettings`) with `disableClaudeAiConnectors` and
  `autoMemoryEnabled: false`, the latter even with `connectors`, and
  `CLAUDE_CODE_DISABLE_AUTO_MEMORY=1` in its environment and that file's `env`,
  as an inherited `0` would turn auto-memory on ahead of the setting. Codex:
  `-c features.apps=false`, then one `-c mcp_servers={…}` turning off every
  server of the layers its run reads (Codex's system layer, the account's
  `config.toml`, the project's `.codex/config.toml` from its root down, found
  as Codex 0.169.0 finds them: `codex/config.rs` `helper_mcp`), one it calls
  `luna` too unless Luna gives the run its own; then Codex merges the two key
  by key, and a `luna` setting more than Luna's replaces stops the start. Then
  one `-c plugins={…}` turning off each plugin on in those layers but the
  library's (`plugins_on`), skills and all, and `_default` for the ones no
  layer names (`codex/launch.rs` `connectors_off`). A table's keys are quoted,
  so a name with a dot is turned off like any other; a table given whole
  replaces what earlier `-c`s set under it, so these come before Luna's own
  server and the library's plugins. A layer that will not read stops the
  start, as only a server some layer defines may be named, and so does a
  managed layer setting servers, plugins or apps, which no `-c` can turn off.
  A chat the window starts is never a helper
  (`ensure_*_session` registers it with no parent), so it keeps its account's
  servers, as one started by hand would.

## Rights

| Tool | May target |
| --- | --- |
| `luna_session_list`, `luna_session_get` | the caller's own; from a person's chat, with `scope: "all"` every session of this launch, and with `ids` any session's row, as metadata: another's `ask`, `askId` and `task` only with `scope: "all"`. A helper names only itself and its subtree (`ids_in_reach`) |
| `luna_run_list` | the caller's spawns and its helpers' (by owner now, else the recorded lineage); everyone's with `scope: "all"`, from a person's chat |
| `luna_message_list`, `luna_report_get` | its descendants; from a person's chat with `scope: "all"` any session but its own |
| `luna_session_send` | its descendants; from a person's chat with `scope: "all"` the person's other chats too, the text opened `[From session <id> via Luna]` (`as_sent`), never another's helper (`write_reach`); `interrupt` descendants only; command text (`/`, `!`) only into its own helpers, whatever mode it runs in (`command_reach`) |
| `luna_session_wait` | its descendants; from a person's chat with `scope: "all"` any session but its own |
| `luna_session_stop`, `luna_session_delete` | descendants only, never one the person is opening as a chat (`promoting`) |
| `luna_session_answer` to a model's question | its descendants; from a person's chat with `scope: "all"` the person's other chats too, an answer typed in as a message opened as `luna_session_send`'s (`answer_message`), never another's helper (`write_reach`) |
| `luna_session_answer` with `allow`, or on a CLI's own list | descendants only, whatever mode either runs in |
| `luna_session_spawn` | a background helper in the mode it names, or with none passed, never in or above the data dir; a Codex one never with an approval that asks a person |
| `luna_notification_send`, `luna_power_set` | a person's chat only (`persons_chat_only`): a helper's word reaches the person through the chat above it |

Every descendant is a background helper: a chat the window runs has no parent,
so no agent interrupts, stops, deletes or approves for it.

**A helper reaches only its own subtree**
([ADR-0025](../adr/0025-a-helper-reaches-only-its-own-subtree.md)). Only a
person's chat (`registry.rs` `persons_chat_in`: no parent, not an orphan, not
run in the background; or the dev caller) may say `scope: "all"`, notify the
person or set the PC's power. A helper, a lead with tools included, is refused
`scope: "all"` in every tool (`scope_all_in`, `SCOPE_ALL_REFUSED`), and
`in_scope`, `authorize_view` and `authorize_send` (`beneath`) refuse it any
session not under it, its parent and the person's chats among them, saying
that what is for anyone else goes in its reply. A prompt injected into a
helper deep in a tree stays in that tree.

**Writes into a helper come from above it**
([ADR-0028](../adr/0028-writes-into-a-helper-come-from-above.md)). Text and
answers go into a helper only from its ancestors: its parent, a lead between,
the chat at the top. `registry.rs` `write_reach` decides it, for the tool's
gate (`in_scope`, for the `send to` and `answer` verbs) and for the write
itself (`authorize_send`), and says why it refuses (`WriteRefused`): a
person's chat writing into another's helper (`NotAbove`) hears whose the
helper is, that it may still read and wait on it with `scope: "all"`, and that
the person can move it to that chat in the window; a helper hears
`not_under_you`; a former parent hears the takeover's words. A person's chat
still writes to the person's other chats with `scope: "all"` (R1), and the dev
caller anywhere. A helper with no chat above it (an orphan, one run in the
background with no parent) takes no agent's writes. Whoever writes into a
session is owed its answer, not only its parent: a send or an answer makes a
debt for the caller (`owed.rs` `owe`), and the notice names the session as one
"which you wrote to" (notifier `run`).

Every call's arguments are held to its tool's schema first (`arguments_fit`):
an unknown key is refused naming those the tool takes, a value of the wrong
type or outside its enum is refused, a required one must be there and not
empty. `scope` is checked in `mcp.rs` (`agents::in_scope`) before the tool runs;
an unknown session or the caller itself is left to the tool's own refusal.

Tool depth is 1 (a helper's helpers get no tools). A line, a person's chat and
every session under it, has as many places for sessions at work as Settings →
Helpers says (`Settings::max_helpers`, 16 by default, held to 1–64 by
`max_descendants`); see Places below. The full matrix with code references is in the
[threat model](../security/threat-model.md#5-mcp-authorization-matrix).

**Lineage.** Children outlive their parent (`forget`): each passes to the
parent's own parent; with none it is an orphan (`Known::orphan`, saved in
`Ties`), listed under "Helpers with no chat" (`LooseHelpers`), and joins
`auto`. A helper started without a folder works where its caller does, in the
worktree its CLI made for the caller included (`agents::caller_worktree`: a
read of where the caller works, nothing made). A helper that passes to another
parent (`pass_up`) is told as it next starts.

**Handed over.** A helper the person opens as a chat (`background_promote`,
then the window's `background_promoted`) becomes theirs for good
(`handover::take_over`), in this order:

1. While its record still stands with `promoting` set (no turn starts, sends
   queue, and `authorize` refuses its parent's kill and delete), one hold of
   the registry takes its parent away, every debt anyone was owed by it, its
   seen counts, receipts, delivery floors and breaker counts, its `auto` and
   `borrowed` marks and a helper's settings (`HELPER_SETTINGS`), and records
   `taken_over` (helper → former parent). From then on its former parent's
   send, read and wait are refused as its own (`in_scope`), its kill and delete
   by `authorize`, both with the takeover's words. A `wait` in flight holds to
   its `scope` on every look and before it hands anything over (`wait.rs`), so
   every ancestor's wait without `scope: "all"` ends with nothing of it, and a
   `wait_many` lists it `refused`, with only why (`Waited::refused`); with
   `scope: "all"` it reaches the chat as any of the person's.
2. Then the helper's record is taken (`background_promoted`), with what was
   queued for it and what never reached it, and `drained` moves its count on
   (`handovers`) and clears its debts again: a send whose text went in before
   that checks its debt against the count it took first (`owe_from_at`) and
   registers none, and the notifier drops a notice prepared on a debt gone
   since (`debts_moved`). A notice's delivery and a handover share one
   exclusion per session (notifier `Telling`, `hand_off`): the delivery checks
   its debts under it and holds it until its text is in, the whole handover
   runs under it, so either the notice is in before anything changes hands or
   it finds the handover and goes nowhere. The window's word is acted on once
   (`Entry::handing`); said again, or after the record is gone, it changes
   nothing.
3. Each session owed by it, and each sender of that text, gets one line in
   `Registry::news`: its parent that the person took it over and nothing more
   is owed, the others that nothing more is owed by it; each with the file its
   undelivered text is in (`note_file`). `news` is saved in `chat-ties.json`,
   so the line outlives a core; the dev caller's goes to the log.

The helper's own helpers stay its; the chat the person now has owes them
nothing new. A Codex helper's thread keeps the developer message it was
started with (`codex resume` sends no `developer_instructions`), so the same
hold keeps what it carried apart from its settings (`STARTED_WITH`: its role,
its constraints' words), `background_promoted` adds the thread it ran in
(`started_in`), `register` keeps it across starts, and the pane that resumes
that thread says it in `role.layers-kept` (`carried_into`); a new thread
says nothing. A Claude Code session reads its layers at every start, so a
chat's replace a helper's. A row the window could not make leaves everything as it was.
`agents.jsonl` records the run as ended `taken over by the person`
(`agentlog::TAKEN_OVER`): a history's lineage goes by each link's owner now
while the core knows it, and past a gone link by the spawner its run records,
never past such a run (`mcp.rs` `runs_in_scope`).

**Moved.** The person moves a helper, with every helper under it, to another
of their chats from the window (`background_move`, `reparent::move_helper`;
[ADR-0028](../adr/0028-writes-into-a-helper-come-from-above.md)); no tool
does. Only its parent changes, so its line follows; its id,
settings and constraints stay. It moves only a direct child of a person's chat
or a helper with no chat above it, only to a person's chat with Luna's tools,
not in its own line, whose line has their
places for those of them at work and their spawns under way (`check_target`,
counted as places are, `background/places.rs`: one between turns needs none), and under which
none of them would be deeper than a spawn could make it (`spawnable_at`:
tools only at depth 1, nothing below depth 2; a lead left with no chat may
have spawned a lead). In this order:

1. The notifier's hand-off on each moved session (`hand_off`): a notice about
   one going in is in first, none starts after.
2. Whose the helpers are, alone (`registry::ownership_changing`). Every send
   and answer into anything but a person's chat holds it shared from
   `authorize_send` to the debt it leaves (`delivery::send`, `answer::answer`),
   a helper a spawn has registered and not yet started included; a stop from
   `authorize` until its turn is marked stopped (`teardown::kill`,
   `background::kill_holding`), so a former chat never stops a turn its new
   chat started (an agent's delete asks again under its claim, which a move
   refuses); and a spawn from its
   registration to its caller's debt (`spawn/background.rs`
   `register_spawned`), which asks
   `spawnable_at` again for the helper under that hold and refuses the spawn
   if a move of its caller's line since put it too deep; so one checked
   against the old chat has its debt in
   before the move goes on, and one after is checked against the new chat; a
   helper is never started under the parent it had before a move. A
   background send never takes the notifier's `going_into`, which is why the
   hand-off alone would not do.
3. The helpers between turns (`background::claim_move`, `Entry::moving`, part
   of `held_back`): no turn starts until the move is through, a turn's end
   included (`turn_over` leaves the queue to `go_on`), so the next one starts
   with `LUNA_PARENT_ID` naming the new chat and its queued command text is
   judged against the new owner; one running goes on. A turn claimed but
   not yet started is let start first if its command is made
   (`Run::starting`: the move waits for its process, up to 15 s, and it goes
   on as one begun before the move); if not, it is `Run::overtaken` and starts
   nothing: its batch goes back to the head of the queue for `go_on` after the
   move, which reserves it again and judges its command text against the new
   owner. A turn on text that is no batch (an interrupt, a spawn's first) is
   claimed and started under `ownership_steady`, which the move waits on. A
   helper being opened
   as a chat, deleted or moved
   refuses the move; while it moves it refuses those. Under this claim the
   helper's work and its turn count are read in one look
   (`background::work_and_count`), so a turn ending meanwhile cannot leave the
   new chat owed an end already past.
4. One hold of the registry checks it all again and changes it. The line is
   looked at afresh there: one that differs from the line the holds were
   taken on (a helper spawned under it since, one gone) changes nothing, and
   the move takes its holds again on the line as it is now, up to three times
   (`move_line`), so no session moves unheld and none a delete holds. Then the
   parent, the orphan mark, an `autoDelete` `pass_up` switched on
   (`Auto::by_pass_up`), and the old chat's debts, requests, receipts, seen
   counts, delivery floors, answer marks and breaker counts on every moved
   session; the new chat owed the helper's answer at its count now if it is in
   a turn, has text queued or holds an ask (`owed::owe_in`), so a chat going
   meanwhile (`forget`, decided in a hold of the same lock) takes that debt
   with it; the old chat's one line in `news`, with how many of its messages
   queued still go in (plain text only: `queued_from`). It records `moved`
   (`Moved`: from, to, when, whether a turn has been told), which its ties
   keep.
5. Then the exclusive hold on whose the helpers are goes, before anything
   that may start a turn; the background spec's parent is set from the
   change (`background::set_parent`): every change of a helper's parent in the
   registry, a move's or a chat's going's (`pass_up`), is counted
   (`Registry::parent_changes`), and the spec takes a parent only from a
   change later than the one it has, so a chat's going that changed the
   registry before the move but writes the spec after it loses; the claim and
   the hand-off go (a claim given up without a move clears the `overtaken`
   it set, `MoveClaim::moved`, so a turn whose launch has not looked yet
   starts as claimed; one whose launch saw it first has put its batch back,
   and `go_on` starts it again under the same parent); the save and the rows'
   event.

From then on the old chat's send, answer, stop, delete, and read or wait
without `scope: "all"` on any moved session are refused naming the new chat
(`moved_away`, `moved_error`): a wait in flight sees it at its next look. Its
waits with `scope: "all"` go on. A wait that took its answer as the move came
puts no debt back (`owed_on_for` puts one in only for a caller still above
it). The record stands for refusals only while the chat moved to is above
the helper: once that chat is gone, or the person opens the helper as a chat
(`change_hands` drops the record and its suggestions), the former chat is
judged as any other, chat to chat by R1. A save takes every helper's ties
in one hold of the registry (`ties_of_each`). The helper's next turn gets a note in its
prompt that the person moved it and to which chat (`move_note`), naming
whom it answers to at that turn, its parent then (a chat moved to and gone
since is named as gone, and no chat as above it), until a
turn's model has answered a prompt that carried it (`move_told`, from
`turn_over` once the turn `acted`, and only if that move is still the last:
the turn's `Run::moved` names it), on both CLIs: Claude Code has its role note
in its first prompt only, Codex beside it every turn. Text queued before the move goes in as
queued: a Claude Code turn running takes it on stdin at a tool boundary, a
Codex helper at its next turn; command-led text from the old chat is dropped
at delivery and the old chat told (`settle_batch`, `queued_refused`).

A person's chat refused a write into a helper by the write rule (`NotAbove`)
is recorded with when (`Registry::suggested`, in memory, `asked_to_move`)
against the helper the line moves with (`movable_root`: the helper itself, or
for a helper's helper the chat's own helper above it), and that row offers it
("Move to <chat>?"); the refusal names the same helper. A move or the
helper's going clears it. A refused read or wait, or a helper refused, records nothing. The
rows (`background_helpers`) carry `top` (the person's chat the line hangs
from; none when its top is a helper whose chat was deleted, `ownership_of`),
`watchedBy` (the person's chats with a `wait` in flight on it that are not
above it, `Registry::waits`), `moved` and `suggested`; the window asks
`background_move_targets` for each chat's verdict before it offers a move,
and shows "Move to <chat>?" only for a chat that may take it now.

**Deleted.** Every delete of a helper, an agent's, the person's or `reap`'s,
first holds it (`background::claim_delete`): no turn starts and it cannot be
opened as a chat until the delete has removed it or let it be, and an opening
under way refuses the delete. An agent's delete asks `authorize` again under
that hold, so ownership and idleness are what it removes; `remove` refuses a chat the person took over.

## Spawning

Every helper runs in the core (`spawn/background.rs`, [background
helpers](background-helpers.md)); the window takes no part. `spawn` settles the
account, folder and role (`settle_role`; `reaches_luna`), records the
spawn on disk (`SpawnUnderWay`), registers the helper with its parent
(`spawn_is` notes its task), starts its first turn and answers with `id`,
`settings`, `turnsEnded`, `sameFolder` (other live sessions in its folder) and
`places`. Luna makes, inspects and removes no worktree
([ADR-0030](../adr/0030-scope-of-0-20.md)): a caller that wants isolation
makes one with `git worktree add` and passes it as `folder`, and `luna_session_spawn`
refuses a `worktree` argument saying so (`mcp/args.rs` `worktree_instead`).

**Places.** The cap is the line's, on its helpers at work, and it is
checked in one place: where a helper's turn is claimed, under the helpers'
own lock (`background/places.rs`, `room_for_turn`, `claim_queued`). A line is
its top (a person's chat, or a helper with no parent) and every helper below
it by `Spec::parent`. A place is taken by a helper with a turn claimed or
running (`Entry::run`: its record's `running` or `asking`), and by a spawn
under way (`promise_place`); each session counts once, as its spawn's
promise is taken over by its first turn in the hold that makes the helper
(`start_on`). A helper between turns takes none, whatever its turns left
running (shells, crons, a job that would not list): those are the power
rule's to weigh, and no turn of its CLI runs. A Claude Code lead whose CLI
stays up past its answer for its background tasks keeps its run, and its
place, until that process goes: its background subagents are model work,
and the turns its CLI starts by itself at a task's end pass no check of
Luna's. Every descendant is a background helper, so a session with no
helper record yet is one being spawned, which its promise covers.

Every claim passes the check: a send to an idle helper (`place`), the next
turn on what was queued as a turn ends (`turn_over`), what waited behind a
stop between turns (`end_left_marked`) or a hold (`go_on`: a move letting go,
an opening refused, a power-off called off), an interrupt of an idle helper,
and a turn a core's going cut short, resumed after a restart (`resume`) by
the number of places the settings say then. With no place free the turn is
not refused: its text stays queued, the helper is marked waiting
(`Entry::waits_since`, in the order marks are given), and a send says so
(`Placed::Waits`, `waitsForPlace`), its answer still the next turn's end.
Turns waiting start in that order, line by line, as places free: `wake`, on
every `changed` (a turn's end, a delete, a stop, a move letting go), a
spawn's place given back and a change to the number of places, runs `go_on`
for each waiting turn that has a place, on a thread of its own, one pass at a
time. A turn that has waited longer is ahead of one asked for later, a spawn
included: a spawn is refused while the turns at work, the spawns under way
and the turns waiting fill the line. The exception, by design: a turn on
Luna's own text (a notice or a wake, `Queued::from` none: a helper's answer,
a question it asks) starts past the limit, as a lead whose line is full of
its own helpers would otherwise never hear them.

A spawn is refused early, before folders and logins are read (`room_now`),
and again where its place is promised (`SpawnUnderWay::begin`); the agents'
layer reads the lines off its own lock (`background::lines`), as the helpers'
lock is never taken under the registry's. `places` (`Places`, in the spawn's
result and in `luna_session_list`) is `inUse` (helpers at work and spawns
under way on the caller's line) of `of`, `underYou` (those under the caller
itself, and its spawns) and `waiting` (turns waiting for a place). A refusal
says what takes a place, the spawns under way and turns waiting, and names
the caller's own at work, to wait for or stop (`free_first`); below the top
it says the rest are the top chat's to free, and the top chat hears it
(`refused_a_place`, kept in `Registry::places_refused`): one line per burst
of refusals, riding its next notice or going alone after `RIDE_MAX_MS` as
other riders do (`riders_go`), cleared once that notice is in
(`places_told`); a refusal after that starts a new line. A move takes a
helper's line to a chat only with places there for those of them at work and
their spawns (`check_target`); their next turns pass the check there. The
number of helpers a line keeps between turns is not capped: each is a
registry entry and its files, with no CLI process (but what its turns left
running, which `luna_session_stop` ends).

**Mode when not given.** A spawn that names no mode (Claude: no
`permissionMode`; Codex: no `sandbox`) is started with none
(`background_spec`, `Spec::own_mode`): no `--permission-mode` for Claude Code,
which runs in what its settings say (shown as Luna read them,
`claude/defaults.rs`). A Codex helper gets no sandbox override and its
`settings` say `sandbox: "codex"`: Codex's own, from its config, a value Luna
does not resolve or claim (`codex/defaults.rs` `OWN`), and it gets no deny of
Luna's `core/` either. The exception is a config Luna cannot read: any layer
Codex might read, trusted or not, that will not parse or names an approval,
sandbox or profile Luna does not know makes it `read-only`, passed
explicitly. Both carry `modeFrom: "cli"`. A mode named is spelled exactly as
the CLI takes it, and is not compared with the caller's: Luna ranks no modes
(ADR-0030, amendment of 2026-11-20). Claude Code helpers start in `auto` and
`dontAsk` too (`CLAUDE_MODES`: `claude -p --permission-mode` takes both,
2.1.322); a Codex helper's approval is `never` (`-c approval_policy="never"`)
or `auto-review`, as nobody is there to ask.

`account` may be a list of one provider's accounts: the first signed in with
every window under 95%, else the one with most room; `accountChoice` says why.
Only the windows the provider reported count, whatever their name: one it did
not send is neither room nor full, and an account with none at all is taken
like one whose usage is unknown.
`readOnly: true` takes Edit, Write and NotebookEdit from Claude Code (whose Bash
can still write) and puts Codex in `read-only` (or `read-only-net`). Claude's
"read-only" is a list of tools, not an OS sandbox: Luna never confines Claude
Code, and a session is held only by its permission mode and its rules, unless
its user turns on Claude's own sandbox (none on native Windows).

A session cannot drive the core: nothing in `<data>/core/` lets anyone in, and
the core refuses a client its OS marks confined, Codex's sandbox among them
(threat model F3, [ADR-0021](../adr/0021-core-endpoint-by-os-identity.md)).
`core/` is still kept from sessions where the CLI can say so, a second wall
(off Windows the core's socket is there):
- Claude Code: every settings file Luna writes (`claude_settings`) denies
  `Read(//<core>/**)` to its file tools (`claude_read_deny`; a Windows drive
  as `//e/…`). A rule, not a sandbox: Bash, where it runs unasked, reads
  anything.
- Codex off Windows: `read-only` and `read-only-net` run under a profile of
  Luna's, named afresh at every start, that extends `:read-only` and denies
  `core/` (`codex/launch.rs` `denies_core`, `profile_args`), the path in the
  TOML value so its dots survive Codex's `-c` key split. `workspace-write`
  (and `auto-review`) stays Codex's own mode, as `:workspace` drops an
  account's `[sandbox_workspace_write]` roots and network.
- Codex on Windows: no deny. The unelevated sandbox refuses every command
  under a profile with one; its restricted token is refused the core's pipe
  by the pipe's DACL.
`agent: "<name>"` is Claude Code's `--agent`; Codex refuses it.

## State and asks

Tool answers carry one `state` (`state_of`: `working`, `waiting`, `question`,
`background`, `idle`, `starting`, `exited`), read off the session's lifecycle
record ([power and lifecycle](power-and-lifecycle.md#the-lifecycle-record);
a record not known yet reads `working`), and an `ask` (`ask_of`): a
background helper's held call; for Claude Code the tool its hooks name
(`PreToolUse` for `AskUserQuestion`, `PermissionRequest`), else a held tool call
in the transcript; for Codex the rollout's open question or an approval its
screen shows (`codex_command_on`): the dialog is found by its last `1. ` row
that has a `2. ` row below it, and its command by the `$ ` row right under the
heading "Would you like to run the following command?", read down to those
options, so a `1. ` line of the command (a heredoc's) does not end it and an
earlier `$ ` row on the screen does not start it. A command the screen does
not hold whole (its end not drawn, its start off the top, two dialogs on it)
says so in the detail, in square brackets.

A permission's `detail` is the one input field that says what the tool would
do (a command, a path, a URL), whole up to `DETAIL_MAX` (8 KB), past it cut on
a character's boundary and ending in `… [N more characters cut]`, with the
count also in `detailCut` (absent when whole, so a command that itself ends in
such words is told from a cut one) (`Ask::permission`, for both CLIs): a
parent allows what it reads there, so it is never cut without a mark. It is
one field, not the input: a `Write`'s content, a `Task`'s prompt and an MCP
tool's other arguments are not in it. The notice line (`Ask::line`) quotes 200
characters of it, each line's whitespace runs made one space and each line
break shown as `⏎`, and marks a longer one
`[shortened; luna_session_get's ask.detail has it whole]`. Invisible
characters that are not whitespace (zero-width ones, a braille blank) are not
collapsed, so padding made of them can still push the end out of the 200; the
mark is there then too. Where Claude Code
says who made the call, `source` names it: `{by: "session"}`, or
`{by: "subagent", agentId, agentType}` for a native subagent whose call the
session's own transcript does not hold. A terminal's comes from the
`PermissionRequest` hook's `agent_id` / `agent_type`; a background helper's
ask on its stdout (`can_use_tool`) names only its `tool_use_id`, matched to the stream
message that made the call (`background::events::made_calls`), whichever comes
in first. A Codex screen names nobody.

A helper's ask is answered on its record (`background::answer`), checked against
the very ask it lands on; asks carry an opaque `id`, which `luna_session_answer` takes
as `askId`. In a chat's terminal (`scope: "all"`) only a question its model
asked is answered, as keys: an option is arrows down and Enter, free text in the
row after the options, several Claude Code questions end on a review Enter
submits; an async question is answered by a message. A permission there is the
person's. `send` into a session on a blocking dialog refuses.

## Reading and waiting

- `read` comes from the transcript (Claude Code: user/assistant without
  `tool_result`; Codex: `response_item` messages) with a byte cursor over whole
  lines. A result is held to 60,000 characters (`MAX_TEXT`; Claude Code cuts an
  MCP result past 25,000 tokens): from `since` it keeps the oldest messages
  that fit and moves the cursor to where they end; with `last` the newest;
  `leftOut` counts the rest.
- `screen` is the pty's output drawn by `vt100` at the pty's size, the
  terminal kept per session (`pty::Drawn`): the last 40 lines a person would
  see.
- `wait(turn_done)` counts turn ends (the record's `turnsEnded`, `activity::Life`) and
  returns everything said past the debt's transcript offset, report first;
  past `MAX_TEXT` it names a file with the whole answer. `wait_many` (`ids`,
  `mode: any|all`) is one `wait` per id on its own thread.
- `send` waits up to 12 s after Enter for a turn to begin.
- `read` and `wait` follow a session onto a new transcript: what is owed is read
  from the start of a fresh one or the end of one gone back to (`read.rs`
  `moved_to`; Claude Code's `/clear` and `/resume`, Codex's `/new` and the like).
- `read` and `wait` on timeout return `starting` for a session that has not drawn
  yet. A chat's CLI started again after a restart is as blank, however far its
  count has gone (`still_starting` goes by this process's turns), and for its
  first minute a notice waits for its prompt too (`drawn_for_notice`).
- `wait(turn_done)` on a helper stopped on a prompt returns `waiting` with the
  screen, since the turn will not end; for 8 s after a `send` from the same
  caller into it the prompt counts as answered.
- A `wait` on a session with the caller's text held waits for it to go in and
  then for the turn it starts, not the `afterTurn` given (`Owed::late`); a session
  that exits first is announced as such (`Owed::lost`).
- The page sends chat names (and window groups) as `chat_names` on every change,
  for `luna_session_list`.
- `list` adds per helper: `task`, `Signs` (turn start, last activity, model, `context` via
  `Context::of`), `leftRunning`, `deletesInMin`, `settings`. Codex's model is
  the rollout's `turn_context` of a turn its model actually answered.
  `defaults` per account come from `claude::defaults::flags` /
  `codex::defaults::flags`; `usage` is what the window last sent
  (`account_usage`): the core never calls the limits endpoints for agents.

## Debts: what a caller is owed

A helper does not report on itself. `spawn`, `send` and `answer` record an
`Owed` (caller, session): `turnsEnded` at the time and the transcript offset
before the text went in (0 for a spawn). Asked again while a debt is open, the
caller is owed both (`min` offset, new baseline).

Invariants:

- Only an outcome that carries the answer settles a debt: a `wait` ending
  `turn_done` past the baseline, or a `read` that began at or before the
  offset, returned everything uncut, ends on an assistant message, and found
  the session idle with nothing of its own running. A timeout only restarts the
  autoDelete clock (`looked`).
- Text typed into a chat's terminal mid-turn may join that turn or start the
  next, so a `queued` debt counts a turn end only once no turn has begun 8 s
  later (`QUEUED_GRACE_MS`, `queue_settled`).
- Every debt has a revision (`Owed::rev`); `take_debt` looks and takes in one
  hold of the lock, and `settle_told` and `job_done` go by the revision.
- Each tool call has an `mcp::Call` whose fate is decided once by
  `compare_exchange`: Running→Committed when `wait`/`read` hands the answer
  over (`hand_over` returns an undo), Running→Canceled on cancel. A cancel that
  crosses the response (an answered call stays findable for 10 s,
  `LATE_CANCEL`) runs the undo. A cancel never loses the answer; at worst it
  arrives twice.
- A `wait` its caller lets go hands nothing over: the MCP cancel notification,
  the caller's session gone, or for a Codex caller its turn over with no other
  begun for 8 s (`LET_GO_MS`).
- `wait_many`: finished ones stay owed and reserved from the notice
  (`reserved`) until the response; a cancel undoes them together except an
  answer a later call delivered (`receipts`).

## The notifier

A thread looks at debts every 2 s. When the caller is between turns on two
looks in a row (idle, not starting, not typed into for 20 s) it types one line
`[Luna notice, not typed by the user] …` with Enter: the helper finished a
turn, stopped on a prompt (once per stop) or exited.

- The answer is everything said past the debt's offset (`replies_since`),
  written to `<data>/agents/<id>-<sha256, 128 bits>.md` (named by content),
  report first (`report_of`: the longest message of the last turn, or with every
  one under 400 characters the last; when that short turn is one the CLI started
  by itself (`Said::by_cli`, a background job's end), the longest message of the
  latest turn before it that wrote one of 400 or more (`Said::starts`)), then the last, then the rest. The notice quotes about
  150 characters of the report's first line (`opening_line`), not for a failed
  turn, and the report's lines in the file (`report_span`: `lines 1-N of M`).
  `luna_report_get` (`read::report_get`) gives that report alone, from `since`,
  else the debt's offset, else the transcript's start, with `rest` and a file of
  the whole; it starts the autoDelete count over (`looked`), and with `ack` a
  report of all of an owed answer, its turn over, hands it over as a whole read
  does (`hand_over`, undone by a cancel); else nothing. It reads as a read does: its cursor stops before a line still
  being written, the report is cut at 60,000 characters, and a `since` that finds
  nothing finds nothing (only a read with no cursor falls back to a resumed
  conversation's last word).
- A `wait` still out for that answer (`waits`) delivers it instead; if it has
  not within 10 s the notice goes (Claude Code moves an MCP call past 120 s to
  the background and may drop it).
- A turn after which the helper still runs processes waits up to 20 s, then
  says "NOT done yet" once per debt (`Owed::told_pending`), naming what is left.
  After that the caller hears only of a turn no notice started (`notice_turn`,
  `real_turn_past`) or the job's end (`Verdict::JobOver`), however long the
  job does nothing: what it does is not measured, and idle is no proof of an
  end. "What is left" and "the job's end" are the session's record's
  (`activity::Life`, `TurnState::children_busy`): Claude Code's shells, crons
  and subagents by its hooks and registry, a Codex chat's shells in its job, a
  helper's processes in its job. A job's end is clocked on the debt
  itself (`Owed::held`): the notifier and a `wait` go by the same 20 s
  (`held_for_wait`, `job_end_stood`), so the end reaches the caller once.
- A notice is an ordinary turn of the caller; a dev caller with no terminal gets
  none, its lines going to the log. Typing one takes seconds and the turn it
  starts may ask again meanwhile, so the notifier changes the debt only if it is
  still the one it announced.
- Lines no debt brings (`Registry::news`: a helper taken over, an auto-delete, a
  queued text refused) wait for a caller a restart has yet to bring back for as
  long as any caller is waited for (`CALLER_RETURN_MS`), and are saved with
  `chat-ties.json` until told.
- **Riders.** A job's end with nothing said (`Settled::NoNews`), the idle line,
  a context line (`budget_news`) and a refusal for want of places are no reason
  to wake a caller: they ride with the next notice (`riders_go`), and go alone
  only once they are all it is owed, nothing in Luna is busy, or after
  `RIDE_MAX_MS`. Quota news does not ride: its lines go into the notice's lines
  as they are put together (mod.rs, before `riders_go` is asked), like a turn's
  end, so a window at 90% wakes the caller and brings the riders with it.
- **Company.** A notice ready for an idle caller that is owed an answer by
  another session still mid-turn (`Turn::Busy`) waits for it up to
  `COALESCE_MS` (20 s; `hold_for_company`, counted from the first look it was
  ready at, on the monotonic clock), so helpers ending seconds apart wake the
  caller once. Past that, or with nothing else at work, it goes as it would
  have; one that carries a question or a permission prompt (`Settled::Waiting`,
  `Settled::Asked`) never waits.
- A helper with its own helpers out (`waits_on_helpers`) reads `background`; a
  turn on a notice from them counts once it has all their answers.
- **Leads.** A lead (`is_lead`: spawned with tools) whose delegates still work
  (its own helpers out, or what its CLI left in the background) is not done as
  a matter of course: a turn it ends then settles as `Settled::Skip` (no
  notice, `told_pending` set, the debt's offset kept, where that turn's text
  ends kept as `Owed::held_to`), so what it wrote rides in its report with
  the next notice, never "NOT done yet". Not the first
  turn to answer a send of its caller's (`Owed::by_send`, before
  `told_pending`): that is the answer asked for, unless the send asked for
  none. A message whose
  first line opens on `DONE`, `BLOCKED`, `QUESTION`, `FAILED` or `PLAN`
  (`said::says_status`, case-sensitive, alone or before `:` or a space,
  markdown marks aside) goes at once (`worth_waking`), and so does a turn on
  its helpers' notices that wrote one (`news_for_parent`, only ever upward).
  Prompts, questions, errors, exits and the quiet-turn notices go as ever.
  The lead's role text says so (`resources/system/role-lead.md`).
  Once all its delegates are done (`Verdict::JobOver`, or a turn on its own
  helpers' notices once they have all answered, `turns_on_helpers`) with
  nothing said of them (`quiet_on_helpers`, `quiet_since_held`: that last
  turn wrote nothing, or nothing since `held_to`; what it wrote while they
  worked waits for its parent and is no word on their end),
  Luna wakes the lead instead of telling its parent (`wake_lead`). Whether
  its interim turn was held back first is no matter: a helper that answers
  within that turn's reply grace ends before the notifier has looked at it.
  A notice never drops what a session wrote because its last turn wrote
  nothing (`written_since`, `replies_since` stopped). The wake is a queued
  line opening `Your delegates have finished` (`WAKE_HEAD`), naming what it
  was last seen waiting on (`note_delegates`), once per end (`Registry::woken`,
  by the count it was seen at; a notice of the lead's end to another of its
  callers, told without a wake, leaves it standing until the lead has ended
  a turn past it, notifier `settle_delivered`), counted by the breaker as the
  pair (lead, lead). The turn it starts is marked `NoticeTurn::wake` and counts as the
  lead's own; if it ends with nothing said since the wake, the parent hears
  the lead "seems stuck" (once: it settles the debt). A wake not taken up
  within `WAKE_WAIT_MS` (10 min), or one the breaker stops, leaves the parent
  told as before.
- **Nothing running.** A helper's or lead's turn end told to its parent as
  "finished its turn" adds "Nothing of its own is running: it will not go on
  unless you send to it." (`NOTHING_RUNNING`) when nothing would start its
  next turn (`runs_nothing`): idle with nothing its turns left
  (`children_busy`: what its job holds, both CLIs; a Claude Code `-p` turn
  stays open while its background subagents work, and `codex exec`
  interrupts its subagents as the turn ends), no answer owed to it by anyone
  but that parent, no `wait` of its out, nothing queued, held or reserved for
  it (background `work_and_count`), and no line waiting to go in: one queued
  for it (a wake), or one the notifier types in unasked (an account's usage
  window past a level, `quota_due`; a helper's context past its level,
  `budget_due`, its transcript read then; a spawn a core's going cut off; a
  refused place), as each starts a turn of its own. Each is read as it
  stands when the notice is put together, not as the notifier's last look
  saw it, so a usage figure published since can only hold the sentence
  back.
  What it wrote does not count: a lead ended a turn on "Gate is running;
  waiting." with no gate started, and its parent read that as work under way
  (issue #77). Not with a cut-off line, which already says a send has it
  carry on, nor to a caller that is not its parent.
- **Cut-off work.** A helper's background work its CLI stopped, or took with
  it as its turn's process went, is said in every line that settles a
  caller's debt and in the `backgroundStopped` of a wait, read or report,
  once per caller owed an answer then (lines.rs `background_stopped_line`,
  [background helpers](background-helpers.md)), and keeps a lead's wake from
  standing in for the notice. Claude Code subagents the process never said
  were done go by name: "2 background subagents ("Implement slice A",
  "Implement slice B") ended with its turn without finishing; their work is
  not done" (`subagents_lost`, the first five named), never "finished": a
  lead's parent read "its delegates finished" while the implementers its
  lead's CLI took with it had committed nothing. Codex's are worded the same
  ("1 subagent ended with its turn …"), named where their rollout names
  them. After Luna's own stop of the turn the line says only that they
  were cut and their work is not done, with no advice on how to work.
  Each turn's loss is its own, with its own cause: a caller hears each it
  has yet to hear of, once, a woken lead's "seems stuck" line included,
  and a reply acknowledges those it said, never one recorded since.
  Claude Code's own stop with none named, and a Codex stop an older
  core saved uncounted, keep their own lines (`BACKGROUND_STOPPED`,
  `SUBAGENTS_INTERRUPTED`).
- **Requests.** Each (caller, session) numbers the caller's requests (owed.rs
  `Requests`: a spawn 1, each send one more, with its `requestKey`), and a
  debt covers those made since the last one settled. With more than one,
  `Said::asked` puts each message down to the latest request whose text (its
  first 60 characters, `request_head`) opened a prompt at or before it; the
  notice says it answers several and which the quoted report is for
  (`answering`, `begins_re`), and `answer_text` sets each request's part apart
  under `Re request N`, the latest first, as do `luna_report_get` and
  `luna_session_wait` (`report_text`). Kept in memory only: after a core
  restart an answer names no request.
- **No reply asked.** A send with `expectReply: false` sets `Owed::no_reply`
  (a later send that asks for one clears it). A turn that answers it wakes the
  caller only with a status word or a message of 400 characters or more;
  otherwise the debt settles with no notice (`Settled::Unasked`). A send that
  takes in a debt a notice may be telling of as it comes keeps what it owes
  of its own past each debt it took in, the last four (`Owed::since`, by
  `rev`): once that notice settles the debt it was put together on, whichever
  that was, what is left is what came after it alone, from where the first
  ask after it was asked, wanting a reply only if one of them did, with their
  requests (owed.rs `told_before`), so an earlier answer is not told again.
  The rest is the debt as it stands: the last ask's baseline and what was
  decided of it (`queued`, `late`, `unsure`, `cut_short`). A session gone on
  in another transcript has every offset its debts keep moved there
  (read.rs `moved_to`: the cursor, `held_to` and each `since`'s).
- **The breaker.** Past 12 turn-end notices in 15 minutes for one (caller,
  session) pair (`BREAKER_NOTICES`), that pair's turn and job news stops: the
  last notice says so, the log warns, the desktop is told once, and a person
  typing into either chat lifts it. Prompts, questions and exits are never
  silenced (`standing_line`). This ended a night of 460 notices an hour, in
  which every notice started a turn whose end re-owed the answer.
- **Quota news.** An account a caller's live helpers run on reaching 90%, then
  95%, of a window, once per level per window, kept in
  `<data>/quota-told.json`.
- **Context.** Every fifth look `budget_news` reads each helper's context
  (`Context::of`) against its level (`level_of`): its spawn's `budgetPct` (0:
  never), else its deprecated `budgetTokens` as the share of the window they
  are, else the person's setting (`Settings::helper_context_pct`, 50 by
  default, 0 off). Past it, `Registry::context_seen` holds the reading and
  every notice line about the helper ends ` (context N%)` (`context_rider`).
  Past 85% (`ALARM_PCT`), or its level if higher, the first reading stays as
  `budgetBreach` and a line of its own rides to the parent once, advising a
  fresh helper. Nothing is stopped.
- **The log.** Each notice's line names, per session told of, what became of
  it (`kind_of`, the `Settled` variant) and how long it was held for company
  (`Notice::held_for`): what a wider batching window would have saved is read
  off there.

## Typing into terminals

- Luna types one thing at a time per terminal (`TypingInto`): notices on their
  own threads, held sends on `luna-held`, `send` and `answer` in the call's
  thread (waiting up to 25 s for their turn; a send not let in is held, an
  answer refused). A second Enter for one the CLI missed never goes over keys
  typed since. `type_text` holds a person's keys back (`hold_keys`) from its
  look at the draft to the Enter; they go on after, into an empty composer.
- **Drafts.** `write_session` counts a likely draft from keystrokes
  (`pty::draft_after`); once keys settle (1.5 s) `has_draft` reads the composer
  off the screen (`pty::read_composer`); for a Codex chat it reads it with no
  keys counted too, as Up and Ctrl+R recall a line of history into the
  composer without a key typed there. In Claude Code a real draft is put
  aside (`stash_and_type`: Ctrl+S, text, Enter; the CLI's "Draft restored" is the
  sign it went), unless a stash is already shown (`› stashed`). Codex has no
  stash (0.169.0's keymap has none: Tab queues the draft itself, Ctrl+S is the
  history search's next match; `puts_draft_aside`), and a draft cut out of its
  composer by other keys does not come back with its pastes and images: so a
  notice or send is never typed over it. The text is held (`Registry::held`, `deliver_held`, `held: true`), as is
  a send into a chat typed in within 5 s. A draft or text held a minute is
  announced on the desktop once. `sessions_meta` carries `draft` and `held` for
  the row tag and the pane balloon.
- Terminals are the person's chats only: what types into one is a notice to a
  chat with tools, a `luna_session_send` with `scope: "all"`, or an answer to a question
  its model asked. Nothing presses a key for an agent there.
- `send` with `interrupt: true`, to the caller's descendants only (every one a
  helper): its turn is killed and its held asks cleared, and the text is the
  next turn.
- A message past 8,000 characters reaches a chat as a file
  under `<data>/agents/` (an `--add-dir` for Claude Code sessions with tools or
  an agent above; Codex sandboxes read anywhere, but `read-only` and
  `read-only-net` off Windows are denied `core/` (F3) — `workspace-write` and
  every Windows sandbox still read it, the latter through the data folder's
  private DACL), swept after a week.

## Failed turns

`failure_of`: a background helper's `error` (`result` / `turn.failed`, a
non-zero exit, `luna_session_stop`); Claude Code's last transcript line when it is
`isApiErrorMessage`, else its `StopFailure` hook; Codex's `task_complete.error`
or `turn_aborted` reason (`Scan::failed`); a pty session's exit code. The notice
says "stopped on an error"; `wait` returns `failed`. API-error lines,
injected user lines (`<task-notification>`, interrupt markers, compaction
summaries) and `<synthetic>` replies are not messages (`message_of`). A turn cut
short hands over what it said before. A Claude Code transcript is looked for
under the registry's cwd, then in every project folder (`located_transcript`);
none is `transcriptMissing`, not "no reply".

## autoDelete and history

`reap`, on every notifier pass, deletes a helper with no debt either way, no
`wait` on it, no helper of its own, nothing its turns left still in its job
(its record not active, however long that has done nothing), and no turn for 30 minutes, as `delete` would.
Any `read` / `wait` / `send` restarts the clock (`deletesInMin`).
A helper an agent killed mid-turn is left alone. On by default.

The person's Delete on a helper, or on a chat with its helpers ticked
(`delete_by_user`), refuses one at work (a turn, an ask held, what its turns
left running) unless told to stop it. A worktree it ran in stays as it is.
The dialog asks the core what each has at stake
first (`at_stake`) and offers each as a box of its own. From a chat's dialog
it names that chat (`of_chat`) and refuses a helper whose line hangs from
another chat by then, asked under the delete's hold (`claim_delete`), which a
move's `claim_move` waits on.

Before any delete `remove` writes a `left` line to `agents.jsonl` (transcript
path, last message, cut to 400 characters by `history`).

## Failure modes

| Failure | Behavior |
| --- | --- |
| Hub down | hooks fail open; MCP calls fail |
| Caller's CLI cancels a `wait` | nothing handed over; the notice brings it |
| Two models answering each other | breaker after 12 notices in 15 min |
| Core dies mid-turn | debts, ties and queues restored from disk; callers told "cut short" ([background helpers](background-helpers.md#outliving-the-core)) |
| Window gone while opening a helper as a chat | the next window settles it by whether its row exists (`settleOpenings`) |
| A move refused (no tools, its own line, a lead, places, deeper than a spawn reaches, held back, not a chat's own helper) | nothing changed (but a turn whose launch saw the move's claim first, which starts again under the same parent once the claim goes); the refusal names why, and `background_move_targets` says it per chat beforehand |
| A move waits on a send, an answer or a stop in flight into a helper, or on a notice about it going in | it goes on once that is in, the debt it left closed; a notice still going in after 120 s refuses the move (`hand_off`) |
| Core dies between a move and the helper's next turn | its ties keep the move and that no turn has been told; the next core's first turn of it is |
| A chat's CLI draws a dialog differently | not recognized: it waits for the person |

## Testing without a second Luna

`LUNA_DEV_TOKEN` (+ `LUNA_DEV_FOLDER`, `LUNA_DEV_ACCOUNT`) registers a caller
`dev` with that bearer token; its helpers are listed under "Helpers with no
chat", and its notices go to the log.
`scripts/test-build.ps1 -Run -DevCaller` sets them ([development](../development.md)).
