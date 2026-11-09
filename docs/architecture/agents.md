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
| `mcp/tools.rs` | the sixteen `luna_<resource>_<verb>` tools (`REMOVED`: the old names, refused with the current list); `luna_session_handoff` only for a chat with auto-handoff on (`handoff_tool`); `luna_permission` only for a background Claude Code helper's CLI |
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
| `agents/notifier/` | the notifier loop; `lines.rs` what a notice says; `usage.rs` quota and budget news |
| `agents/handoff/` | auto-handoff: `luna_session_handoff`, the threshold notices (`watch.rs`), the file check and copy (`file.rs`), the switch's machine (`phase.rs`), its saved state (`store.rs`, `<data>/handoffs/handoffs.json`) and what the prompt and notices say (`facts.rs`) |
| `agentlog.rs` | `<data>/agents.jsonl`: a line per spawn and per end |
| `luna_guide.md` | the manual `luna_guide_get` returns a section at a time, by a fixed topic per tool and `notices`, `restart`, `helpers` (`mcp/guide.rs` `TOPICS`), or with no topic its opening and the topics |

## How a session gets Luna

- **Claude Code:** `--mcp-config <data>/mcp/<chat>.json` with
  `"timeout": 1900000` and header `Bearer ${LUNA_MCP_TOKEN}` (the token in the
  environment, never in the file), plus `--allowedTools mcp__luna`.
- **Codex:** `-c mcp_servers.luna.url=…`, `bearer_token_env_var=LUNA_MCP_TOKEN`,
  `tool_timeout_sec=1900`, `default_tools_approval_mode="approve"`: Luna's tools
  ask no permission in any mode; the checkbox is the permission.
- Tokens are per session and reissued on every spawn. The instruction travels
  in `initialize`; together about 2k tokens once.
- Every session has `LUNA_SESSION_ID`; a helper also `LUNA_PARENT_ID`. A helper
  is told what it is and to write in its task's language (`pty::helper_line`),
  naming the words instructions files use for the person (the operator, the
  user, the person at this machine) so that rules set for that person's chat do
  not reach it, and what it still owes of them: to stop, before its turn ends,
  the background work it started that nothing needs any more (watchers, wait
  loops, dev servers; not helpers it waits on, nor what its task keeps
  running), keep each caveat with its claim, and send nothing outward unasked. The library can keep such rules from it altogether (an entry's
  audience, [library](library.md#audiences)). Claude Code by `--append-system-prompt` and a note after its
  first prompt (`pty::helper_prompt`), as memory files outweigh an appended
  system prompt; Codex by the note after each turn's prompt, because
  `-c developer_instructions=` would replace a value Luna would have to resolve
  from Codex's config stack. The transcript readers drop the note
  (`without_note`). Slash-command prompts go without.
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
| `luna_session_list`, `luna_session_get` | the caller's own; with `scope: "all"` every session of this launch, and with `ids` any session's row, as metadata: another's `ask`, `askId` and `task` only with `scope: "all"` |
| `luna_run_list` | the caller's spawns and its helpers' (by owner now, else the recorded lineage); everyone's with `scope: "all"` |
| `luna_message_list`, `luna_report_get` | its descendants; with `scope: "all"` any session but its own |
| `luna_session_send` | its descendants; with `scope: "all"` any session but its own, the text opened `[From session <id> via Luna]` (`as_sent`); `interrupt` descendants only; command text (`/`, `!`) only from full access into its own helper |
| `luna_session_wait` | its descendants; with `scope: "all"` any session but its own |
| `luna_session_stop`, `luna_session_delete` | descendants only, never one the person is opening as a chat (`promoting`) |
| `luna_session_answer` to a model's question | its descendants; with `scope: "all"` any session but its own, an answer typed in as a message opened as `luna_session_send`'s (`answer_message`) |
| `luna_session_answer` with `allow`, or on a CLI's own list | descendants only, at the caller's privilege |
| `luna_session_spawn` | a background helper at no more privilege than the caller, never in or above the data dir; a Codex one never with an approval that asks a person |
| `luna_session_handoff` | the caller's own chat only (no target argument); offered and taken only from a person's chat with Luna tools and auto-handoff on |

Every descendant is a background helper: a chat the window runs has no parent,
so no agent interrupts, stops, deletes or approves for it.

Every call's arguments are held to its tool's schema first (`arguments_fit`):
an unknown key is refused naming those the tool takes, a value of the wrong
type or outside its enum is refused, a required one must be there and not
empty. `scope` is checked in `mcp.rs` (`agents::in_scope`) before the tool runs;
an unknown session or the caller itself is left to the tool's own refusal.

Tool depth is 1 (a helper's helpers get no tools); at most 8 descendants per
chat. The full matrix with code references is in the
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
nothing new. A row the window could not make leaves everything as it was.
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
the worktree once the CLI has made it (`worktreePending` until then).

`account` may be a list of one provider's accounts: the first signed in with
every window under 95%, else the one with most room; `accountChoice` says why.
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
  (`Settled::Idle`); the debt stays open. A job's end is clocked on the debt
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
  quota news and budget news are no reason to wake a caller: they ride with the
  next notice (`riders_go`), and go alone only once they are all it is owed,
  nothing in Luna is busy, or after `RIDE_MAX_MS`.
- **Company.** A notice ready for an idle caller that is owed an answer by
  another session still mid-turn (`Turn::Busy`) waits for it up to
  `COALESCE_MS` (20 s; `hold_for_company`, counted from the first look it was
  ready at, on the monotonic clock), so helpers ending seconds apart wake the
  caller once. Past that, or with nothing else at work, it goes as it would
  have; one that carries a question or a permission prompt (`Settled::Waiting`,
  `Settled::Asked`) never waits.
- A helper with its own helpers out (`waits_on_helpers`) reads `background`; a
  turn on a notice from them counts once it has all their answers. Meanwhile
  its parent hears of turns that wrote a message of 400 characters or more
  (`news_for_parent`), once each, only ever upward.
- **The breaker.** Past 12 turn-end notices in 15 minutes for one (caller,
  session) pair (`BREAKER_NOTICES`), that pair's turn and job news stops: the
  last notice says so, the log warns, the desktop is told once, and a person
  typing into either chat lifts it. Prompts, questions and exits are never
  silenced (`standing_line`). This ended a night of 460 notices an hour, in
  which every notice started a turn whose end re-owed the answer.
- **Quota news.** An account a caller's live helpers run on reaching 90%, then
  95%, of a window, once per level per window, kept in
  `<data>/quota-told.json`.
- **Budget.** `budgetTokens` on a spawn: `budget_news`, every fifth look, reads
  `Context::of`; the first reading past it stays as `budgetBreach` and rides to
  the parent once. Nothing is stopped.

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

## Auto-handoff

A person's chat with Luna tools may have auto-handoff on: `handoff` in its
registry settings, the threshold in percent, passed by the window at every start
(`ensure_*_session`) and set on a running chat by `set_auto_handoff`
([ADR-0020](../adr/0020-session-self-handoff.md)).

- **The tool.** `tools/list` adds `luna_session_handoff` for such a chat alone
  (`offers_tool`: tools, no parent, not an orphan, a helper or the dev caller) and
  notes that its CLI has it (`listed`, cleared by `register` at every start: a
  CLI lists tools as it starts, and Luna's server cannot announce a new one).
  The tool takes `file` and nothing that names a chat. It is refused while the
  CLI's own jobs run (`activity::own_jobs`: what its turns started that is still
  alive, its subagents), and while one is under way.
- **The threshold.** The `luna-handoff` thread, every 2 s: for each opted-in
  chat whose CLI has listed the tool, at a turn count it has not looked at with
  the turn over, `Context::of` its transcript; past the threshold one line in
  `Registry::news` (the notifier types it), past `firm_at` (threshold + 10, kept
  within 85-95) a firmer one; once each per conversation (`Told`, by the
  transcript's name). A conversation a handoff began (`Record::fresh`) keeps
  its first reading as a floor and is asked only past it by `GROWTH` (5
  points): one that begins above the threshold is not sent straight into the
  next handoff. A switch scheduled drops a notice not yet typed.
- **The copy.** `file.rs`: absolute, `.md` / `.markdown` / `.txt`, opened with no
  link followed (and, off Windows, without waiting, so a FIFO swapped in does not
  hang the open) and checked on its handle, at most 128 KB of UTF-8 text, refused
  inside the data dir after `canonicalize`; all of it on a thread of its own
  given 10 s (`READ_WAIT`), outside the records' lock; a reader given up on keeps
  its place until its thread ends (`Readers`: one per chat, four in all, a call
  past either refused): the number is reserved
  first (`Record::reserved`, never saved, one call at a time per chat) and taken
  past any copy already on disk (`next_free`), so one a stopped core left is never
  written into. The copy is made new and private as `<data>/handoffs/<chat>/<n>.md`
  and flushed before the switch is recorded, its SHA-256 on record. Claude Code
  chats with auto-handoff get that folder as an `--add-dir`, read without asking.
- **Recording it.** The switch is recorded on disk (`write_private_synced`; a
  save that fails is undone and refuses the call), only while the reservation
  is still the call's and auto-handoff is still on, both looked at under the
  record's lock (turning it off takes that lock, clears the reservation and
  changes the registry inside it), under the notifier's
  exclusion for the chat (`hand_off`): whatever is going into the chat then (a
  notice, `tell_of`; a held send or a send, `going_into`) goes in first, into
  the conversation that writes the handoff, and from then on each of them, which
  looks again at `holds_text` under the same exclusion, goes nowhere until the
  switch is over.
- **The switch** (`phase.rs`). Every phase is taken by `Phases::advance`, for
  that handoff's number, from the phase it was read in, on disk before any text
  goes in; one not taken (given up meanwhile, a save that fails) types nothing.
  Turning auto-handoff off gives up only a switch still `Scheduled`, under the
  same lock, so it never strands a reset. `Scheduled` (from the call:
  `notice_waits` holds the notifier, `deliver_held` and `send` hold text for the
  chat) → the chat at rest on two looks (`ready_for_notice`), no draft, no job of
  its own → a thread claims the terminal (`TypingInto`) for the rest. Each text
  goes in by `Term::type_line`: a person's keys held (`hold_keys`), the composer
  looked at again (anything in it, a stash, keys within 5 s: not now), the phase
  taken, then the paste and the Enter. → `Resetting`: `/clear` or `/new` typed →
  the CLI on another conversation: the registry's
  `sessionId` (Claude Code, `claude::session::session_id`) or the title's thread
  (Codex, `codex_thread`) → `Prompting`: the prompt typed, `[Luna handoff N, not
  typed by the user]`, the copy's path and `fact_block` (from `list::list`, the
  debts, `held`, `undelivered_for`, `Registry::usage`, the settings); past 6,000
  characters the facts go in `<n>-facts.md` beside it → the transcript other
  than the one left holds that opening → done. Then what waited goes in. A
  person writing in the new conversation before the prompt holds it back
  (`Recovering`) until they are through.
- **Failures.** 60 s without a new conversation, 60 s without the prompt in the
  new transcript, 30 min without rest, 10 min with its CLI not running, the
  CLI's own jobs at rest, or a phase that will not save: given up, kept on record (`Past`), the chat told by a
  line in `news` (before the reset: it goes on as it was, and may call again
  only if the tool is still its; after: what it is, the prompt again) and the
  desktop by a notification. A draft holds it as long as
  it stands, told to the desktop after a minute.
- **A core restart.** `from_bytes`: a switch in `Resetting` or `Prompting` is
  `Recovering`. Once the chat is back and at rest: on the conversation it left,
  `Scheduled` again (two resets at most); on another, done if its transcript
  holds the prompt, else the prompt is typed.
- **The window.** `session_meta` carries the latest `HandoffMark` (under way,
  done, failed) for the pane's divider (`chats/Handoff.tsx`); `handoff_file` and
  `handoff_transcript` read the copy and the conversation left by the record's
  paths (`background::view_of`). A conversation's first prompt that is Luna's
  handoff prompt is no name for the chat (`pty/probe.rs` `pin_name`).
- `forget` drops a deleted chat's record and its folder of copies; a chat keeps
  its last 20 on record.

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
