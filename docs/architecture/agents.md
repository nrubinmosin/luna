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
| `mcp.rs` | JSON-RPC: `initialize` (with `instructions`), `tools/list`, `tools/call`; twelve `luna_*` tools; `luna_permission` only for a background Claude Code helper's CLI; `mcp::Call` per call in flight; `HISTORY_LAST` |
| `agents/registry.rs` | the `Registry` behind one lock (`reg()`): tokens (by SHA-256), `Known` sessions, lineage, pending spawns, debts, waits, receipts, names, groups |
| `agents/owed.rs` | answers owed (`Owed`), `hand_over` and its undo |
| `agents/ask.rs` | what a session waits on (`ask_of`), screen recognition of dialogs |
| `agents/read.rs`, `agents/said.rs` | transcripts and rollouts read with a byte cursor; what a session said and how a result puts it (`reply_of`, `report_of`) |
| `agents/lost.rs` | what never reached a session |
| `agents/privilege.rs` | the least-privilege layer (`settle_mode`, `reaches_luna`, `command_reach`) |
| `agents/delivery.rs` | typing into terminals, `luna_send`, `await_start` |
| `agents/answer.rs`, `wait.rs`, `list.rs` | `luna_answer`, `luna_wait`, `luna_list` / `luna_sessions` |
| `agents/teardown.rs` | kill, delete, `reap` (autoDelete), `forget` |
| `agents/persist.rs`, `agents/resume.rs` | ties across cores; chats carried on after a core |
| `agents/spawn/` | `luna_spawn`: `account.rs` (account lists), `background.rs`, `worktree.rs`, `settle.rs` (spawns that never completed) |
| `agents/notifier/` | the notifier loop; `lines.rs` what a notice says; `usage.rs` quota and budget news |
| `agentlog.rs` | `<data>/agents.jsonl`: a line per spawn and per end |
| `luna_guide.md` | the full manual `luna_guide` returns: whole (`topic: "all"`), by section (`mcp.rs` `guide_text`: any word of a `## ` title), or with no topic its opening and the sections |

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
  not reach it: Claude Code by `--append-system-prompt` and a note after its
  first prompt (`pty::helper_prompt`), as memory files outweigh an appended
  system prompt; Codex by the note after each background turn's prompt (a
  visible one's opening prompt only), because `-c developer_instructions=`
  would replace a value Luna would have to resolve from Codex's config stack.
  The transcript readers drop the note (`without_note`). Slash-command prompts
  go without; a compaction can lose a visible one's.
- The instruction says the CLIs' own subagents cannot use another provider or
  account (without it a Codex orchestrator used its own `spawn_agent`).
- A helper gets Luna's server and none of its account's, unless spawned with
  `connectors: true` (`keeps_connectors`: the spawn's record, then
  `connectors` in its settings, one of `CORE_SETTINGS`). Claude Code:
  `--strict-mcp-config` (no user, project, plugin or claude.ai server) and a
  settings file (`HelperSettings`) with `disableClaudeAiConnectors` and
  `autoMemoryEnabled: false`, the latter even with `connectors`. Codex:
  `-c features.apps=false` and `-c mcp_servers.<name>.enabled=false` for each
  bare-key server of the account's `config.toml` (`connectors_off`), one it
  calls `luna` too unless Luna gives the run its own; then Codex merges the
  two key by key, and an account's `luna` setting more than Luna's replaces
  stops the start. A config that will not read stops it as well. The
  project's `.codex/` and plugins' servers stay, as overriding a server no
  layer defines fails the run. A visible helper is one with a parent at its
  start.

## Rights

| Tool | May target |
| --- | --- |
| `luna_sessions`, `luna_list` | all sessions of this launch (`list` with `ids` gives any session's rows) |
| `luna_send` | its descendants; with `scope: "all"` any session but its own, the text opened `[From session <id> via Luna]` (`as_sent`); `interrupt` descendants only; command text (`/`, `!`) only from full access into its own helper |
| `luna_read`, `luna_wait` | its descendants; with `scope: "all"` any session but its own |
| `luna_kill`, `luna_delete` | descendants only |
| `luna_answer` to a model's question | its descendants; with `scope: "all"` any session but its own, an answer typed in as a message opened as `luna_send`'s (`answer_message`) |
| `luna_answer` with `allow`, `keys`, or on a CLI's own list | descendants only, at the caller's privilege |
| `luna_spawn` | a helper at no more privilege than the caller, never in or above the data dir |

Every call's arguments are held to its tool's schema first (`arguments_fit`):
an unknown key is refused naming those the tool takes, a value of the wrong
type or outside its enum is refused, a required one must be there and not
empty. `scope` is checked in `mcp.rs` (`agents::in_scope`) before the tool runs;
an unknown session or the caller itself is left to the tool's own refusal.

Tool depth is 1 (a helper's helpers get no tools); at most 8 descendants per
chat. The full matrix with code references is in the
[threat model](../security/threat-model.md#5-mcp-authorization-matrix).

**Lineage.** Children outlive their parent (`forget`): each passes to the
parent's own parent (a visible one's row hears `agent://reparent`); with none it
is an orphan (`Known::orphan`, saved in `Ties`): a visible one stays a row
marked ↳, a background one is listed under "Helpers with no chat"
(`LooseHelpers`) and joins `auto`. A helper started without a folder works
where its caller does, worktree included (`agents::caller_worktree`). A helper
that passes to another parent (`pass_up`) is told as it next starts.

## Spawning

The core does not know chats, so a visible spawn is a round trip (sequence in
the [overview](overview.md#an-agent-spawns-a-visible-helper)):

1. `agent://spawn` to the page, which makes the chat (`parentId`, not seated in
   a pane) and calls `ensure_*` with the prompt as the CLI's positional argument
   after `--`.
2. The page answers `agent_spawned`; the core waits on a Condvar up to 90 s,
   then up to 30 s more for the session to take up the prompt (`started`).
   Otherwise the answer carries `screen`, and with the screen still empty
   `starting`: before its first draw Claude Code syncs organization skills and
   makes the worktree, which can take minutes.
3. An event reaches only a listening page, so the request stays on its record
   until a session comes of it (`SpawnTaken::asked`), and the page asks for
   what is waiting once it listens and at every core (`agent_spawns`). The row
   is named by the request, so a reloaded page finds the same row.

While a session is `starting`, its prompt sits in the CLI's arguments and
`send` refuses. `spawn` also reports: an unknown slash command (`started: false`
at once), `warnings` (screen lines with ⚠, e.g. a model the organization does
not allow), and `sameFolder` (other live sessions in the helper's folder).

`account` may be a list of one provider's accounts: the first signed in with
every window under 95%, else the one with most room; `accountChoice` says why.
`readOnly: true` takes Edit, Write and NotebookEdit from Claude Code (whose Bash
can still write) and puts Codex in `read-only` (or `read-only-net`). Claude's
"read-only" is a list of tools, not an OS sandbox: Luna never confines Claude
Code, and a session is held only by its permission mode and its rules, unless
its user turns on Claude's own sandbox (none on native Windows).

The core's token in `<data>/core/core.json` is the key to every command
(threat model F3), so no session is to read it:
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
  under a profile with one, and its write-restricted token reads `core/`
  through the folder's private DACL.
`agent: "<name>"` is Claude Code's `--agent`; Codex refuses it.

## State and asks

Tool answers carry one `state` (`state_of`: `working`, `waiting`, `question`,
`background`, `idle`, `starting`, `exited`) and an `ask` (`ask_of`): a
background helper's held call; for Claude Code the tool its hooks name
(`PreToolUse` for `AskUserQuestion`, `PermissionRequest`), else a held tool call
in the transcript; for Codex the rollout's open question or an approval its
screen shows.

`luna_answer` in a terminal is keys: an option is arrows down and Enter, free
text in the row after the options, several Claude Code questions end on a
review Enter submits; a permission is Enter (Codex `y`) or Esc; an async Codex
question is answered by a message. `keys` takes `{enter}`, `{down}`… for any
other dialog. `send` into a session on a blocking dialog refuses. Background
asks carry an opaque `id`; `luna_answer` takes it as `askId`.

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
  for `luna_sessions`.
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
- Text typed into a visible session mid-turn may join that turn or start the
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
  `luna_read` with `report: true` (`read::report_read`) gives that report alone,
  from `since`, else the debt's offset, else the transcript's start, and hands
  nothing over. It reads as a read does: its cursor stops before a line still
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
  none. Typing one takes seconds and the turn it starts may ask again meanwhile,
  so the notifier changes the debt only if it is still the one it announced.
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
- `send` with `interrupt: true`, to the caller's descendants only: Esc in a
  terminal (up to 3 s for the turn to read as over), then the text as a prompt
  of its own; a background helper's turn is killed and its held asks cleared.
- A prompt or message past 8,000 characters reaches a visible session as a file
  under `<data>/agents/` (an `--add-dir` for Claude Code sessions with tools or
  an agent above; Codex sandboxes read anywhere, but `read-only` and
  `read-only-net` off Windows are denied `core/` (F3) — `workspace-write` and
  every Windows sandbox still read it, the latter through the data folder's
  private DACL), swept after a week.

## Failed turns

`failure_of`: a background helper's `error` (`result` / `turn.failed`, a
non-zero exit, `luna_kill`); Claude Code's last transcript line when it is
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
A helper a person typed into (`typed_ago_ms`) or an agent killed is left alone.
On by default for background helpers, off for visible ones.

Before any delete `remove` writes a `left` line to `agents.jsonl` (transcript
path, worktree, last message, cut to 400 characters by `history`).

## Failure modes

| Failure | Behavior |
| --- | --- |
| Hub down | hooks and guard fail open; MCP calls fail |
| Window not listening for `agent://spawn` | request kept until a page asks (`agent_spawns`) |
| Caller's CLI cancels a `wait` | nothing handed over; the notice brings it |
| Two models answering each other | breaker after 12 notices in 15 min |
| Core dies mid-turn | debts, ties and queues restored from disk; callers told "cut short" ([background helpers](background-helpers.md#outliving-the-core)) |
| A CLI version draws a dialog differently | not recognized: `allow` refused, only raw `keys` reach it |

## Testing without a second Luna

`LUNA_DEV_TOKEN` (+ `LUNA_DEV_FOLDER`, `LUNA_DEV_ACCOUNT`) registers a caller
`dev` with that bearer token; its spawns are top-level chats.
`scripts/test-build.ps1 -Run -DevCaller` sets them ([development](../development.md)).
