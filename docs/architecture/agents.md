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
| `hub.rs` | loopback HTTP: `/hook/<chat>/<secret>`, `/mcp` (bearer); a thread per request in bounded lanes (`admit.rs`: 64 connections reading headers, 32 hooks, 128 MCP calls; a full lane answers 503); the POST is authenticated before its body is read; headers get 5 s, bodies 30 s, a hook body 64 KiB, an MCP body 4 MiB |
| `mcp/mod.rs` | JSON-RPC, one message per POST: `initialize` (with `instructions`; protocol `2025-06-18`, `2025-03-26` or `2024-11-05`), `ping`, `tools/list`, `tools/call`, and the notification that cancels a call (a notification gets 202 and no body); `mcp::Call` per call in flight |
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
| `agents/answer.rs`, `wait.rs`, `list.rs` | `luna_session_answer`, `luna_session_wait`, `luna_session_list` |
| `agents/teardown.rs` | kill, delete, the person's guarded delete (`delete_by_user`, `at_stake`), `reap` (autoDelete), `forget` |
| `agents/persist.rs` | ties across cores: a helper's in `helpers.json`, the person's chats' debts, held sends, spawns under way and untold lines in `chat-ties.json` |
| `agents/spawn/` | `luna_session_spawn`: `account.rs` (account lists), `background.rs`, `folder.rs` (where the caller works, who else is there), `settle.rs` (spawns that never completed) |
| `agents/notifier/` | the notifier loop; `lines.rs` what a notice says; `usage.rs` quota news |
| `agentlog.rs` | `<data>/agents.jsonl`: a line per spawn and per end |
| `luna_guide.md` | the manual `luna_guide_get` returns a section at a time, by a fixed topic per tool and `notices`, `restart`, `helpers` (`mcp/guide.rs` `TOPICS`), or with no topic the offered topics by section |

## How a session gets Luna

Each delegate CLI start reads `Settings::delegate_auto_compact_tokens` before
`background/command.rs` builds its launch. Claude Code gets
`CLAUDE_CODE_AUTO_COMPACT_WINDOW` in its environment, subject to settings-file
`env` overrides. Follow-up turns in the same Claude Code process retain its
launch environment (`background/events.rs`, `LISTEN_MS`). Codex gets
`-c model_auto_compact_token_limit=<n>` on its app-server command line each
turn, including on cold resume. Off omits the override. Luna records what it
applied and keeps the launch inputs for the estimate in `background/rows.rs`,
which reads token usage from the transcript or rollout. Claude Code's
settings are reread when they change. Codex's
snapshot layers the limit above account and project config and below managed
config. A delegate restored after a core restart refreshes its snapshot on
the next turn. PTY chats, including handed-over delegates, start with
their usual CLI settings.

- **Claude Code:** `--mcp-config <data>/mcp/<chat>.json` with
  `"timeout": 1900000` and header `Bearer ${LUNA_MCP_TOKEN}` (the token in the
  environment, never in the file). `--allowedTools` lists the fully qualified
  names from `mcp/tools.rs`, omitting `luna_session_spawn` so it follows
  Claude Code's permission mode.
- **Codex:** `-c mcp_servers.luna.url=…`, `bearer_token_env_var=LUNA_MCP_TOKEN`,
  `tool_timeout_sec=1900`, and `default_tools_approval_mode="approve"`.
  `mcp_servers.luna.tools.luna_session_spawn.approval_mode="prompt"` lets
  Codex handle spawn approval. Luna pre-approves spawn when it passes
  `never` and can rule out managed approval changes. A helper started with
  no named policy saves the policy Codex reports once its first turn starts.
  Any pre-approval based on that saved policy applies only to the resume
  request. A replacement thread prompts and supplies its own policy. The
  app-server relays Luna tool approval elicitations and fallback spawn
  questions as permission asks only if Luna supplied its server to that
  turn's process. The answer uses the current ask ID and approves only that
  call.
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
  Code reads them from `hooks/<id>/CLAUDE.md`: a helper as memory of that
  folder (`--add-dir`) at every turn, a chat at the end of its system prompt
  (`--append-system-prompt-file`) at every start;
  Codex in `-c developer_instructions` (a chat) or its thread's
  `developerInstructions` at every turn's `thread/start` or `thread/resume`
  (a helper), after its config stack's own value.
  The library can keep the person's chat's rules from it altogether (an
  entry's audience, [library](library.md#audiences)). Beside the task, a
  one-line note (`note`, one for leads and helpers, `pty::helper_prompt`) says it
  comes from another agent, not a person, after a Claude Code helper's first
  prompt and every Codex one's: a Codex thread may have compacted its task far
  from its developer message. Its tag is `[A note, not part of the task: …]`,
  naming neither Luna nor any session. The transcript readers drop the note
  (`without_note`): Luna's defaults, this version's and 0.21's
  (`NOTES_BEFORE`), every override note ever served (by
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
| `luna_session_spawn` | a background helper in the mode it names or the effective default, never in, below or above the data dir (links resolved, a worktree's project folder checked too: `agents/admission.rs`); what a Codex one asks comes to its caller |
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
empty; a null counts as left out, and an integer is a whole number from zero
up. `wait` and `delete` need `id` or `ids`. `scope` is checked in
`mcp/dispatch.rs` before the tool runs (`agents::in_scope` for send, answer,
wait and the transcript reads; list, get and the run history check their own);
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
3. Its former parent, each caller with an active or watched debt, and each
   sender of that text gets one line in
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
chat's replace a helper's. Its MCP token goes in the same hold: with no
parent its bearer would pass for a person's chat, so whatever kept it (a shell
its turns left, a copy of its environment) is refused from then on, and the
pane that resumes it is registered with a new one (threat model F17). A call it
made before the hold runs on with no rights: the hub holds the token on the
call's thread (`registry::presenting`), and each right is decided by whether
that token still names the caller, under the lock the right is decided under
(`stands`); a cancel finds only the calls its own token made. A row the
window could not make leaves everything as it was.
`agents.jsonl` records the run as ended `taken over by the person`
(`agentlog::TAKEN_OVER`): a history's lineage goes by each link's owner now
while the core knows it, and past a gone link by the spawner its run records,
never past such a run (`mcp/dispatch.rs` `runs_in_scope`).

The helper rows (`background_helpers`) carry `top` (the person's chat the
line hangs from, when there is one) and `watchedBy` (the person's chats with
a `wait` in flight on the helper that are outside its line).

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
(`spawn_is` notes its task), starts its first turn and returns its `id` and
`sameFolder` (a count of other live sessions in reach working in the same
directory). Wait uses turn 0 as the initial baseline, which spawn records
on the debt. An account list returns `accountChoice`, and an omitted mode
returns its source in `modeFrom`. Luna makes,
inspects and removes no worktree
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
stop between turns (`end_left_marked`) or a hold (`go_on`: an opening refused or a power-off called off), an interrupt of an idle helper,
and a turn a core's going cut short, resumed after a restart (`resume`) by
the number of places the settings say then. With no place free the turn is
not refused: its text stays queued, the helper is marked waiting
(`Entry::waits_since`, in the order marks are given), and a send says so
(`Placed::Waits`, `waitsForPlace`), its answer still the next turn's end.
Turns waiting start in that order, line by line, as places free: `wake`, on
every `changed` (a turn's end, a delete, a stop), a
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
lock is never taken under the registry's. `places` (`Places`, from
`luna_session_get` with `include: ["places"]`) is `inUse` (helpers at work and spawns
under way on the caller's line) of `of`, `underYou` (those under the caller
itself, and its spawns) and `waiting` (turns waiting for a place). A refusal
says what takes a place, the spawns under way and turns waiting, and names
the caller's own at work, to wait for or stop (`free_first`); below the top
it says the rest are the top chat's to free, and the top chat hears it
(`refused_a_place`, kept in `Registry::places_refused`): one line per burst
of refusals, riding its next notice or going alone after `RIDE_MAX_MS` as
other riders do (`riders_go`), cleared once that notice is in
(`places_told`); a refusal after that starts a new line. The
number of helpers a line keeps between turns is not capped: each is a
registry entry and its files, with no CLI process (but what its turns left
running, which `luna_session_stop` ends).

**Mode when omitted.** With no `permissionMode` for Claude Code or neither
`approval` nor `sandbox` for Codex, a mode in the CLI's config gets no mode
override (`background_spec`, `Spec::own_mode`, `settings.modeFrom: "cli"`).
Otherwise Luna passes Claude Code `--permission-mode auto` or Codex's three
Auto-review overrides (`modeFrom: "luna"`). An unreadable or unsupported
Codex layer still takes precedence and gets `on-request` and `read-only`
(`modeFrom: "closed"`). A mode named is validated and applied without
comparing it with the caller's. Codex approval requests come to the caller
as held asks, except when a reviewer model answers them
([background helpers](background-helpers.md#asks)).

`account` may be a list of one provider's accounts: the first signed in with
every window under 95%, else the one with most room; `accountChoice` says why.
Only the windows the provider reported count, whatever their name: one it did
not send is neither room nor full, and an account with none at all is taken
like one whose usage is unknown.
`readOnly: true` takes Edit, Write and NotebookEdit from Claude Code (whose Bash
can still write) and puts Codex in `read-only` (or `read-only-net`); it is
refused with Claude Code's `acceptEdits`, Codex's `auto-review` or any other
Codex sandbox named (`agents/spawn/role.rs`). Claude's
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
  `mode: any|all`) is one `wait` per id on its own thread, at most 16 distinct
  ids. A wait lasts `timeoutSec`, 600 s unless set, held to 1 to 1,800 s.
- `send` waits up to 12 s after Enter for a turn to begin.
- `requestKey` (spawn, send, delete; at most 128 characters): the core keeps a
  finished call's result for up to an hour (64 keys a caller, 1,024 in all,
  16 KiB a result and 2 MiB in all, past which only the outcome), by caller,
  handover generation, tool and key; the same call again gets that result,
  other arguments under the key or a call still running are refused, and a
  failure frees the key. A core restart forgets them (`mcp/dispatch.rs`
  `keyed`).
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
- The window sends chat names and boards as `chat_names` when they change and
  when it joins a new core. `luna_session_list` uses them.
- Session get returns `task` and on request `Signs` (turn start, last activity, model, `context` via
  `Context::of`), `leftRunning`, `deletesInMin` and `settings`. Codex's model is
  the rollout's `turn_context` of a turn its model actually answered.
  `defaults` per account come from `claude::defaults::flags` /
  `codex::defaults::helper_defaults`; `usage` is what the window last sent
  (`account_usage`): the core never calls the limits endpoints for agents.
  Each send is numbered by its window's run (`sender`, `seq`), as the
  window's calls may reach the core out of order; one older than the last
  taken from that sender is dropped (a count per sender). Whichever window
  sent it, the sends are merged per account (`merged`): the reading taken
  later (`takenAt`) stands, and an account a send has with no time (a window
  just started, nothing read yet) or leaves out keeps a dated reading held,
  unless the window says it was just made (`made`: made again at its name,
  it keeps nothing of the last one's) or it is no longer an account (the
  accounts listed as the send is taken; where they cannot be read, none left
  out is kept).

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
  answer a later call delivered (`receipts`). Partial responses retain
  separate receipts for the request IDs each collected.

## Session tool results

`luna_session_list` returns `me` and rows with `id`, `name`, `state`, `turnsEnded`
and `askId` when available. `parent` names an owner other than the caller.
The page includes `nextCursor` and unknown IDs when applicable.
Filters still read the registry fields before checking state and paging.

`luna_session_get` returns identity, state, `turnsEnded`, and the caller-visible
task and full ask. `include` requests extra fields by group. `settings` adds
effective settings, `paths` adds folder, cwd and transcript, `diagnostics` adds
session IDs and version, window group, activity, context, background work and
the auto-delete countdown, and `places` shows the caller's line capacity.
All groups use the same session reach checks.

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
  150 characters of the report's first line (`opening_line`) on successful
  turns. `Report <path>:1–N` locates the report (`report_span`). Short replies
  are quoted whole. A notice gives one request's number and key or the count
  and latest number and key for several. Matched replies still have separate
  `Re request N` sections in the file, newest first.
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
- After a turn leaves work running, a status notice can name it with
  `[helpers pending]`, `[job running]` or a subagent count. Activity comes from
  the session record (`activity::Life`, `TurnState::children_busy`): shells,
  crons, subagents and the helper job's processes. The notifier and `wait`
  share a 20-second grace on the debt for a job's end
  (`held_for_wait`, `job_end_stood`).
- A notice is an ordinary turn of the caller; a dev caller with no terminal gets
  none, its lines going to the log. Typing one takes seconds and the turn it
  starts may ask again meanwhile. A notice, wait or complete read settles the
  request numbers it covered, scoped to the transcript and each request's
  delivery state. A notice acknowledges the restart interruptions and loss
  IDs carried in its rendered text. If Enter produces no observed turn and
  the composer still holds the text, Luna keeps the debt and marks the draft.
  With an empty or unreadable composer, Luna acknowledges the write and
  leaves the turn unmarked, so later notices can proceed. Caller notice
  expiry leaves a report cursor available for a transcript line still being
  written.
- Lines no debt brings (a helper taken over, an auto-delete, a queued text
  refused, or a system notice) wait for a caller a restart has yet to bring back for as
  long as any caller is waited for (`CALLER_RETURN_MS`), and are saved with
  `chat-ties.json` until told.
- **Wake policy.** A fresh `DONE`, `BLOCKED`, `QUESTION`, `FAILED` or `PLAN`
  opening wakes an owed caller after the turn ends. A prompt, question,
  failure or exit can also wake it. A status or answer covers a numbered
  request after the CLI confirms delivery or its transcript shows the prompt. An untold request
  that wants a reply gets one idle stall, carrying what the session said.
  Work, queued inputs and due replies keep that stall back. A peer's due
  reply holds it even while the peer is idle, including an actionable status
  from an FYI. Callers inside the same idle
  cycle can get their own stalls. Callers outside the cycle wait for those
  due replies. The notifier samples the dependency graph once per tick and
  reuses transcript, failure, and activity checks across notices. A draft holds only its
  own notice. Interim and suppressed text
  stays readable. After a PLAN, the idle stall stays covered through that
  turn and later turns started by FYIs or a lead's delegate notices. A
  joined FYI stays part of the original turn. A turn seen before its prompt
  is written is reconsidered if its transcript grows. For eight seconds
  after a silent or skipped end, a status appended past the last complete
  line the notifier read can still wake the caller. Permission prompts,
  failures, exits and restart
  interruptions still go to the caller. A tripped breaker allows the first
  failure of a pair and leaves later failures owed for another outcome or
  when the breaker lifts. A failure or restart cut after a
  silent settlement wakes that caller for a later PLAN turn or a turn
  started by one of its requests. A separate allowance per task covers
  the 30-minute quiet check, including when a turn waits for its own background
  work.
  Ties save request intent and coverage, idle plan state, quiet allowance,
  the transcript boundary confirmed by the last notice, and untold restart
  interruptions. A move to another transcript captures
  the destination offset and invalidates settlements from the old file.
  Silent debts are saved separately so older cores skip them.
- **Riders.** Later quiet lines accompany another notice. A place refusal
  can go alone after ten minutes or when the top chat has no active debt.
  An untold refusal holds the power-off countdown for up to ten minutes
  while its top chat can take it. A claimed refusal holds it through power
  admission. It expires with that chat or its line.
  Suppressed answers hold no countdown. Quota and system notices keep
  their own policy.
- **Company.** A notice ready for an idle caller that is owed an answer by
  another session still mid-turn (`Turn::Busy`) waits for it up to
  `COALESCE_MS` (20 s; `hold_for_company`, counted from the first look it was
  ready at, on the monotonic clock), so helpers ending seconds apart wake the
  caller once. Past that, or with nothing else at work, it goes as it would
  have; one that carries a question or a permission prompt (`Settled::Waiting`,
  `Settled::Asked`) never waits.
- A helper with its own helpers out (`waits_on_helpers`) reads `background`; a
  turn on a notice from them counts once it has all their answers.
- **Leads.** Turn ends while delegates work follow the same rule as for helpers.
  The skipped turn's transcript end (`Owed::held_to`) distinguishes its interim
  text from an answer after its delegates finish.
  Once all its delegates are done (`Verdict::JobOver`, or a turn on its own
  helpers' notices once they have all answered, `turns_on_helpers`) with
  nothing said of them (`quiet_on_helpers`, `quiet_since_held`: that last
  turn wrote nothing, or nothing since `held_to`; what it wrote while they
  worked waits for its parent and is no word on their end),
  Luna wakes the lead instead of telling its parent (`wake_lead`), unless
  a recorded PLAN is awaiting approval. Whether
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
  "finished its turn" adds `[idle; send to resume]` (`NOTHING_RUNNING`) when
  nothing would start its
  next turn (`runs_nothing`): idle with nothing its turns left
  (`children_busy`: what its job holds, both CLIs; a Claude Code `-p` turn
  stays open while its background subagents work, and a Codex turn's server
  goes, its subagents with it, as the turn ends), no answer owed to it by anyone
  but that parent, no `wait` of its out, nothing queued, held or reserved for
  it (background `work_and_count`), and no line waiting to go in, such as
  a wake or a spawn a core's going cut off. Each is read when the notice is
  put together.
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
  and a reply acknowledges the losses it included. An outstanding loss can
  wake the caller after the answer has settled. A launch that fails before
  opening the prompt records the undelivered batch before it publishes the
  turn end. Loss collection waits for the request number and delivery ID
  before exposing a send's lost text. Interrupting sends keep their
  replacement's ID, so that failure settles exactly the requests in the batch.
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
  `luna_session_wait` (`report_text`). A preview with no prompt match is
  marked `[unmatched]` when there are multiple ordinary prompts. A pending
  reply carries its prompt match past the read cursor. Session ties save the
  count, open request heads, and pending match. A legacy debt stays
  unnumbered until settled. Legacy or downgraded numbering may repeat once.
- **No reply asked.** `expectReply: false` wakes only for a status, ask, or
  failure. It retains the existing task's wake state. A send that wants a
  reply starts a new task. A send that
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
  session) pair (`BREAKER_NOTICES`), that pair's turn and job news stops. A
  chat caller hears in the last notice. A lead gets a short warning in
  that notice and its top chat gets the details while the breaker is active.
  A core restart clears the breaker and any pending trip details. The log
  warns, the desktop is told once, and a person
  typing into either chat lifts it. Prompts, questions, exits and a pair's
  first failure still come. Other failures count toward the breaker and
  are held when it trips. A loss-only notice counts once for the IDs it
  acknowledges, with the same allowance for the first failure after a trip.
- **System news.** The person's top chat hears when an account used by live
  delegates at any depth reaches 90%, then 95%, once per account, level and
  window (`<data>/quota-told.json`). Breaker lines, folder-skills failures and places notices
  also go to the top chat. Folder-skills news is told once until the cause
  changes or clears. News can wait for a top chat to register after a restart.
  With no chat above the delegates there is no recipient. Leads
  still hear their delegates' outcomes, including failures on a usage limit.
- **Context.** The pane meter and on-demand `luna_session_get` are the two
  places Luna shows context use (`background::Context`). Its basis is the CLI's estimated
  auto-compaction point where resolved, and otherwise the model window.
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
  (`pty::draft_after`), never from a write marked `reply` (the native
  window's terminal answering the program's queries, which goes straight to
  the PTY, past a key hold too; taken so only for one terminal answer whole,
  `terminal_reply`, anything else so marked going as keys); once keys settle
  (1.5 s) `has_draft` reads the composer off the screen (`pty::read_composer`);
  for a Codex chat it reads it with no keys counted too, as Up and Ctrl+R
  recall a line of history into the composer without a key typed there. In
  Claude Code a real draft is put
  aside (`stash_and_type`: Ctrl+S, text, Enter; the draft coming back, its
  "draft restored" toast, is the sign the text went), unless a stash is
  already shown (`› stashed`, `draft stashed` from Claude Code 2.1.330): the
  CLI keeps one, and Ctrl+S on text overwrites it. A stash that does not show
  within 1.5 s gets as long again to empty the composer and 0.5 s more for
  its mark, and Ctrl+S is pressed a second time, to
  bring the draft back, only on an empty composer. Codex has no
  stash (0.169.0's keymap has none: Tab queues the draft itself, Ctrl+S is the
  history search's next match; `puts_draft_aside`), and a draft cut out of its
  composer by other keys does not come back with its pastes and images: so a
  notice or send is never typed over it. The text is held (`Registry::held`, `deliver_held`, `held: true`), as is
  a send into a chat typed in within 5 s. `sessions_meta` carries `draft` and `held` for
  the row tag and the pane balloon.
- Terminals are the person's chats only: what types into one is a notice to a
  chat with tools, a `luna_session_send` with `scope: "all"`, or an answer to a question
  its model asked. Nothing presses a key for an agent there.
- `send` with `interrupt: true`, to the caller's descendants only (every one a
  helper): the text goes behind any already queued, its held asks are
  cleared, and its turn is asked to stop, killed where the CLI cannot be
  asked; the answer may be `queued` while the old turn is still ending
  ([background helpers](background-helpers.md)).
- A message past 8,000 characters reaches a chat as a file
  under `<data>/agents/` (a `Read` allow rule in a Claude Code chat's
  `--settings` when it has tools, an `--add-dir` for a Claude Code helper with
  tools or an agent above; Codex sandboxes read anywhere, but `read-only` and
  `read-only-net` off Windows are denied `core/` (F3) — `workspace-write` and
  every Windows sandbox still read it, the latter through the data folder's
  private DACL), swept after a week.

## Failed turns

`failure_of`: a background helper's `error` (Claude Code's `result`, a Codex
server's `error` or failed `turn/completed`, a refused request of Luna's, a
non-zero exit; a turn Luna stopped, by `luna_session_stop` or an interrupting
send, is no failure: a wait carries that in its `stopped` field instead);
Claude Code's last transcript line when it is
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
A silent settlement keeps its unread cursor without holding up a lead.
The notifier marks unread answers before checking whether their caller can
take a notice. Unread text holds auto-delete until it is read or 24 hours elapse, even
after the notifier stops waiting for a caller to reopen after a restart.
A helper an agent killed mid-turn is left alone. On by default.

The person's Delete on a helper, or on a chat with its helpers ticked
(`delete_by_user`), refuses one at work (a turn, an ask held, what its turns
left running) unless told to stop it. A worktree it ran in stays as it is.
The dialog asks the core what each has at stake
first (`at_stake`) and offers each as a box of its own. From a chat's dialog
it names that chat (`of_chat`) and refuses a helper whose line hangs from
another chat by then, asked under the delete's hold (`claim_delete`).

Before any delete `remove` writes a `left` line to `agents.jsonl` (transcript
path, and the last message up to 3,000 characters). `luna_run_list` shows up
to 160 characters of a run's prompt and 240 of its last message, 10 runs
unless asked, 50 at most.

## Failure modes

| Failure | Behavior |
| --- | --- |
| Hub down | hooks fail open; MCP calls fail |
| Caller's CLI cancels a `wait` | nothing handed over; the notice brings it |
| Two models answering each other | breaker after 12 notices in 15 min |
| Core dies mid-turn | debts, ties and queues restored from disk; callers told "cut short" ([background helpers](background-helpers.md#outliving-the-core)) |
| Window gone while opening a helper as a chat | the next window settles it by whether its row exists (`Workspace::settle_openings`) |
| A chat's CLI draws a dialog differently | not recognized: it waits for the person |

## Testing without a second Luna

In the test build, `LUNA_DEV_TOKEN` (+ `LUNA_DEV_FOLDER`, `LUNA_DEV_ACCOUNT`)
registers a caller `dev` with that bearer token; its helpers are listed under
"Helpers with no chat", and its notices go to the log. The registration is
compiled into the test build alone (`luna_test`, registry.rs
`register_dev_caller`), and so is what the id `dev` stands for: every check
of the dev caller asks `registry::is_dev`, false outside `luna_test`. A
release has no code that reads these variables for it, and a session given
the id `dev` there is an ordinary one, so no environment and no session can
have a caller that stands in for the person.
`scripts/test-build.ps1 -Run -DevCaller` sets them ([development](../development.md)).
