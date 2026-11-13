# Agents: MCP tools, registry, notifier

Sessions that run sessions. A chat started with **Luna tools** gets Luna as an
MCP server and can spawn helpers, talk to them, answer what they ask and delete
them. User-facing behavior is in the [user guide](../user-guide/agents.md);
decisions in [ADR-0012](../adr/0012-agents-over-mcp-with-notices.md) and
[ADR-0007](../adr/0007-least-privilege-for-agent-tools.md).

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
| `hub.rs` | loopback HTTP: `/hook/<chat>/<secret>`, `/guard/<chat>/<secret>`, `/mcp` (bearer); a thread per request in bounded lanes (`admit.rs`) |
| `mcp/mod.rs` | JSON-RPC: `initialize` (with `instructions`), `tools/list`, `tools/call`; `mcp::Call` per call in flight |
| `mcp/tools.rs` | the sixteen `luna_<resource>_<verb>` tools (`REMOVED`: the old names, refused with the current list); `luna_session_handoff` only for a person's chat with Luna tools (`handoff_tool`, agents/handoff `may_list`), refused to any other as a tool there is not (`dispatch.rs`); `luna_permission` only for a background Claude Code helper's CLI |
| `mcp/args.rs`, `mcp/dispatch.rs`, `mcp/guide.rs` | strict arguments (`arguments_fit`); each call to the agent layer and its log line; the guide by topic (`TOPICS`) |
| `agents/registry.rs` | the `Registry` behind one lock (`reg()`): tokens (by SHA-256), `Known` sessions, lineage, spawns under way, debts, waits, receipts, names, groups, helpers taken over |
| `agents/owed.rs` | answers owed (`Owed`), `hand_over` and its undo |
| `agents/ask.rs` | what a session waits on (`ask_of`), screen recognition of dialogs |
| `agents/read.rs`, `agents/said.rs` | transcripts and rollouts read with a byte cursor; what a session said and how a result puts it (`reply_of`, `report_of`) |
| `agents/lost.rs` | what never reached a helper |
| `agents/privilege.rs` | the least-privilege layer (`settle_mode`, `reaches_luna`, `command_reach`) |
| `agents/delivery.rs` | `luna_session_send`, typing into a chat's terminal (`TypingInto`, `type_text`, the draft put aside) |
| `agents/held.rs` | text held for a person's draft (`hold`, `deliver_held`) and text a core went in the middle of typing (`reconcile_held`) |
| `agents/handover.rs` | a helper the person opens as a chat, handed over for good (`take_over`) |
| `agents/answer.rs`, `wait.rs`, `list.rs` | `luna_session_answer`, `luna_session_wait`, `luna_session_list` / `luna_session_list` |
| `agents/teardown.rs` | kill, delete, the person's guarded delete (`delete_by_user`, `at_stake`), `reap` (autoDelete), `forget` |
| `agents/persist.rs` | ties across cores: a helper's in `helpers.json`, the person's chats' debts, held sends, spawns under way and untold lines in `chat-ties.json` |
| `agents/spawn/` | `luna_session_spawn`: `account.rs` (account lists), `background.rs`, `worktree.rs`, `settle.rs` (spawns that never completed) |
| `agents/notifier/` | the notifier loop; `lines.rs` what a notice says; `usage.rs` quota and context news |
| `agents/handoff/` | auto-handoff ([its own page](handoff.md)): `luna_session_handoff`, the instructions served (`served.rs`), the threshold notices (`watch.rs`), the file check and copy (`file.rs`), the switch's machine (`phase.rs`), its saved state (`store.rs`, `<data>/handoffs/handoffs.json`) and what the prompt and notices say (`facts.rs`) |
| `agentlog.rs` | `<data>/agents.jsonl`: a line per spawn and per end |
| `luna_guide.md` | the manual `luna_guide_get` returns a section at a time, by a fixed topic per tool and `notices`, `restart`, `helpers` (`mcp/guide.rs` `TOPICS`), or with no topic its opening and the topics; every session with tools reads it, so it has nothing of auto-handoff |

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
  privilege is still the mode's (ADR-0007).
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
  `-c features.apps=false` and `-c mcp_servers.<name>.enabled=false` for each
  server of the account's `config.toml` (`connectors_off`), one it
  calls `luna` too unless Luna gives the run its own; then Codex merges the
  two key by key, and an account's `luna` setting more than Luna's replaces
  stops the start. A config that will not read stops it as well, and so does
  a server whose name a `-c` key cannot spell (a dot, a character outside
  letters, digits, `_` and `-`): it could not be turned off. The
  project's `.codex/` and plugins' servers stay, as overriding a server no
  layer defines fails the run; `servers_left_on` (codex/config.rs) lists them,
  plugins as Codex 0.167.0 resolves them across the account's and the project's
  layers (`plugins_on`),
  in the spawn's `mcpLeftOn` and as `helper.mcp-left-on` in "What Luna
  applied". A chat the window starts is never a helper
  (`ensure_*_session` registers it with no parent), so it keeps its account's
  servers, as one started by hand would.

## Rights

| Tool | May target |
| --- | --- |
| `luna_session_list`, `luna_session_get` | the caller's own; from a person's chat, with `scope: "all"` every session of this launch, and with `ids` any session's row, as metadata: another's `ask`, `askId` and `task` only with `scope: "all"`. A helper names only itself and its subtree (`ids_in_reach`) |
| `luna_run_list` | the caller's spawns and its helpers' (by owner now, else the recorded lineage); everyone's with `scope: "all"`, from a person's chat |
| `luna_message_list`, `luna_report_get` | its descendants; from a person's chat with `scope: "all"` any session but its own |
| `luna_session_send` | its descendants; from a person's chat with `scope: "all"` any session but its own, the text opened `[From session <id> via Luna]` (`as_sent`); `interrupt` descendants only; command text (`/`, `!`) only from full access into its own helper |
| `luna_session_wait` | its descendants; from a person's chat with `scope: "all"` any session but its own |
| `luna_session_stop`, `luna_session_delete` | descendants only, never one the person is opening as a chat (`promoting`) |
| `luna_session_answer` to a model's question | its descendants; from a person's chat with `scope: "all"` any session but its own, an answer typed in as a message opened as `luna_session_send`'s (`answer_message`) |
| `luna_session_answer` with `allow`, or on a CLI's own list | descendants only, at the caller's privilege |
| `luna_session_spawn` | a background helper at no more privilege than the caller, never in or above the data dir; a Codex one never with an approval that asks a person |
| `luna_notification_send`, `luna_power_set` | a person's chat only (`persons_chat_only`): a helper's word reaches the person through the chat above it |
| `luna_session_handoff` | the caller's own chat only (no target argument); offered only to a person's chat with Luna tools, and answered only while that chat has an open ask (the threshold's, the person's Hand off now, a move's); to any other, a tool there is not |

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

Every call's arguments are held to its tool's schema first (`arguments_fit`):
an unknown key is refused naming those the tool takes, a value of the wrong
type or outside its enum is refused, a required one must be there and not
empty. `scope` is checked in `mcp.rs` (`agents::in_scope`) before the tool runs;
an unknown session or the caller itself is left to the tool's own refusal.

Tool depth is 1 (a helper's helpers get no tools). A line, a person's chat and
every session under it, has as many places as Settings → Helpers says
(`Settings::max_helpers`, 8 by default, held to 1–32 by `max_descendants`);
see Places below. The full matrix with code references is in the
[threat model](../security/threat-model.md#5-mcp-authorization-matrix).

**Lineage.** Children outlive their parent (`forget`): each passes to the
parent's own parent; with none it is an orphan (`Known::orphan`, saved in
`Ties`), listed under "Helpers with no chat" (`LooseHelpers`), and joins
`auto`. A helper started without a folder works where its caller does,
worktree included (`agents::caller_worktree`). A helper that passes to another
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

**Deleted.** Every delete of a helper, an agent's, the person's or `reap`'s,
first holds it (`background::claim_delete`): no turn starts and it cannot be
opened as a chat until the delete has removed it or let it be, and an opening
under way refuses the delete. An agent's delete asks `authorize` again under
that hold, so ownership, idleness and the worktree it went by are what it
removes; `remove` refuses a chat the person took over.

## Spawning

Every helper runs in the core (`spawn/background.rs`, [background
helpers](background-helpers.md)); the window takes no part. `spawn` settles the
account, folder and privilege (`settle_mode`, `command_reach`), records the
spawn on disk (`SpawnUnderWay`), registers the helper with its parent
(`spawn_is` notes its task and, for a Codex one in a worktree of its own, the
worktrees there were), starts its first turn and answers with `id`,
`settings`, `turnsEnded`, `sameFolder` (other live sessions in its folder) and
the worktree once the CLI has made it (`worktreePending` until then), and
`places`.

**Places.** The cap is the line's: every session under the top chat, every
generation, and every spawn under way with no session yet, counted in the hold
that records a spawn (`under_cap`, `cap_room`); an `autoDelete` helper whose
answer reached its caller takes none. `places` (`Places`, in the spawn's result
and in `luna_session_list`) is `inUse` of `of` on the caller's line and
`underYou`, those under the caller itself. A refusal names the sessions the
caller can delete itself (`free_first`); below the top it says the rest are
the top chat's to free, and the top chat hears it (`refused_a_place`, kept in
`Registry::places_refused`): one line per burst of refusals, riding its next
notice or going alone after `RIDE_MAX_MS` as other riders do (`riders_go`),
cleared once that notice is in (`places_told`); a refusal after that starts a
new line.

**Mode when not given.** A spawn that names no mode (Claude: no
`permissionMode`; Codex: neither `sandbox` nor `approval`) takes, with
Settings → Helpers → "Helpers' mode when not given" at "The spawning chat's"
(`HelperMode::Inherit`, the default), the caller's own mode on the same CLI,
and across CLIs the highest helper mode its privilege covers on the one scale
(`privilege.rs` `inherited`, `CLAUDE_LADDER`, `CODEX_LADDER`): a Codex chat
that asks a person gives `never` with its sandbox; a Claude Code `acceptEdits`
chat a Codex `read-only`. "The account's default" (`HelperMode::Account`)
takes the account's and project's default, as before. Either way the mode is
held to the caller's (`cap`) and pinned; `settings.modeFrom` says `caller`,
`callerNearest` or `account`. The dev caller has no mode and always takes the
account's. Claude Code helpers start in `auto` and `dontAsk` too
(`CLAUDE_MODES`: `claude -p --permission-mode` takes both, 2.1.322).

**One worktree at a time per repository.** Each CLI makes a helper's worktree
with `git worktree add` as its first turn starts, and two of those in one
repository at once failed with "unable to stat just-written file". A spawn
with `worktree: true` holds its repository's queue (`worktree_queue`, keyed by
the common git folder, `worktree::repository_key`) from starting the turn
until the CLI has said where its worktree is, the turn has ended without one,
or `WORKTREE_WAIT` is up.

**Looking into a helper's worktree.** `luna_session_get` on a helper in a
worktree adds `lastCommit` (its subject) and `status` (`git status
--porcelain`, the same lines as `--short`, at most 20, `statusMore` the rest;
`worktree::work_in`): a parent whose CLI keeps it out of other worktrees can
see uncommitted work all the same.

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
`background`, `idle`, `starting`, `exited`) and an `ask` (`ask_of`): a
background helper's held call; for Claude Code the tool its hooks name
(`PreToolUse` for `AskUserQuestion`, `PermissionRequest`), else a held tool call
in the transcript; for Codex the rollout's open question or an approval its
screen shows.

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
- `screen` is the pty scrollback run through `vt100` at the pty's size: the last
  40 lines a person would see.
- `wait(turn_done)` counts busy→idle transitions (`activity` `turns_ended`) and
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
- `list` adds per helper: `task`, `branch` and `sha` (`worktree::head_at`, off
  the `.git` files), `Signs` (turn start, last activity, model, `context` via
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
  `real_turn_past`) or the job's end (`Verdict::JobOver`). All of it idle for 30
  min (`IDLE_JOB_MS`): one notice that only idle processes are left
  (`Settled::Idle`), with what the transcript started them as (`idle_line`:
  Claude Code's open background calls, `claude::session::background_calls`;
  Codex's `exec_command` calls whose output said `Process running with session
  ID N` and no `write_stdin` to it since said it exited,
  `codex::session::background_calls`); the debt stays open. A job's end is clocked on the debt
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
  nothing said past `held_to` (`quiet_since_held`: what it wrote while they
  worked waits for its parent and is no word on their end),
  Luna wakes the lead instead of telling its parent (`wake_lead`): a queued
  line opening `Your delegates have finished` (`WAKE_HEAD`), naming what it
  was last seen waiting on (`note_delegates`), once per end (`Registry::woken`,
  by the count it was seen at; a notice of the lead's end to another of its
  callers, told without a wake, leaves it standing until the lead has ended
  a turn past it, notifier `settle_delivered`), counted by the breaker as the
  pair (lead, lead). The turn it starts is marked `NoticeTurn::wake` and counts as the
  lead's own; if it ends with nothing said since the wake, the parent hears
  the lead "seems stuck" (once: it settles the debt). A wake not taken up
  within `WAKE_WAIT_MS` (10 min), or one the breaker stops, leaves the parent
  told as before. A lead mid-switch to a fresh conversation
  ([handoff](handoff.md#leads)) is not woken, its parent waiting on, and as
  the fresh one starts a wake queued or given in the one it leaves is dropped
  (`lead_goes_fresh`): the opening says where its delegates stand.
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
  of its own (`Owed::since`): once that notice settles the debt it was put
  together on, what is left is the send's alone, from where it was asked,
  with its own `no_reply` and requests (owed.rs `told_before`), so the first
  answer is not told again.
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
  fresh helper unless it hands off by itself (`hands_off_by_itself`: a lead
  while auto-handoff for leads is on, handoff `lead_hands_off`). Nothing is
  stopped. A lead's reading goes as its fresh conversation starts and is not
  taken while its switch is under way: the rider comes back with the fresh
  one's share.
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
  off the screen (`pty::read_composer`). In Claude Code a real draft is put
  aside (`stash_and_type`: Ctrl+S, text, Enter; the CLI's "Draft restored" is the
  sign it went), unless a stash is already shown (`› stashed`). Codex has no
  stash: the text is held (`Registry::held`, `deliver_held`, `held: true`), as is
  a send into a chat typed in within 5 s. A draft or text held a minute is
  announced on the desktop once. `session_meta` carries `draft` and `held` for
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

## Handoff

A person's chat with Luna tools is offered `luna_session_handoff`, which
answers only once it is asked for a handoff: by the person's Hand off now, by
auto-handoff past its threshold, or by a move. On its word it is switched to a
fresh conversation in place; no other session hears of any of it. How:
[handoff](handoff.md).

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
`wait` on it, no helper of its own, nothing left running that still holds the
machine (`left_holds`), and no turn for 30 minutes, as `delete` would, without
its worktree. Any `read` / `wait` / `send` restarts the clock (`deletesInMin`).
A helper an agent killed mid-turn is left alone. On by default.

The person's Delete on a helper, or on a chat with its helpers ticked
(`delete_by_user`), refuses one at work (a turn, an ask held, what its turns
left running) unless told to stop it, and keeps a worktree with unsaved work
(`worktree::unsaved_work`) unless told to drop it, saying so
(`UserDeleted::worktree_kept`). The dialog asks the core what each has at stake
first (`at_stake`) and offers each as a box of its own.

Before any delete `remove` writes a `left` line to `agents.jsonl` (transcript
path, worktree, last message, cut to 400 characters by `history`).

## Failure modes

| Failure | Behavior |
| --- | --- |
| Hub down | hooks and guard fail open; MCP calls fail |
| Caller's CLI cancels a `wait` | nothing handed over; the notice brings it |
| Two models answering each other | breaker after 12 notices in 15 min |
| Core dies mid-turn | debts, ties and queues restored from disk; callers told "cut short" ([background helpers](background-helpers.md#outliving-the-core)) |
| Window gone while opening a helper as a chat | the next window settles it by whether its row exists (`settleOpenings`) |
| A chat's CLI draws a dialog differently | not recognized: it waits for the person |

## Testing without a second Luna

`LUNA_DEV_TOKEN` (+ `LUNA_DEV_FOLDER`, `LUNA_DEV_ACCOUNT`) registers a caller
`dev` with that bearer token; its helpers are listed under "Helpers with no
chat", and its notices go to the log.
`scripts/test-build.ps1 -Run -DevCaller` sets them ([development](../development.md)).
