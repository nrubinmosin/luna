# Background helpers

What `luna_session_spawn` makes, every time: no chat row, no pty, all in the core, so no
window is needed. The person watches one read-only or opens it as a chat of
their own, which hands it over for good. Also: how helpers, and what agents are
owed by the person's chats, outlive a core restart.

## Responsibility

- Run each turn as one headless CLI process and parse its event stream.
- Queue, join and reserve text sent to a helper; hold its permission prompts
  and questions for `luna_session_answer`.
- Track what its turns leave running, and end it with the helper.
- Persist helpers and agent ties so the next core can take them up.

## Key modules

| Path | What it holds |
| --- | --- |
| `background/mod.rs` | the helpers' registry |
| `background/types.rs` | what a helper is made of (`Spec`, `Entry`, `Observed`) |
| `background/queue.rs` | text queued, joined and reserved; command text re-checked against its sender |
| `background/command.rs` | a turn's argv (golden-tested) |
| `background/run.rs` | a turn's process, `OutReader` |
| `background/events.rs` | the CLIs' JSON streams and a turn's end |
| `background/control.rs` | kill, interrupt, leftovers |
| `background/ask.rs` | log as screen, transcript, held asks |
| `background/rows.rs` | `agent://background` rows, the read-only view (`background_transcript`), the person's delete (`background_delete`, `background_at_stake`), opening one as a chat |
| `background/persist.rs` | `helpers.json` across cores |
| `agents/persist.rs`, `agents/held.rs` | chat ties, text held for the person's chats |
| `agents/handover.rs` | a helper opened as a chat, handed over to the person |

## A turn

- One process per turn: `claude -p --output-format stream-json --verbose` or
  `codex exec --json`, resumed by the session / thread id the first turn
  printed, prompt on stdin (no command-line limit). The conversation lands in
  the CLI's own transcript or rollout.
- Busy is the process running, not stdout open: a job the turn left may have
  inherited the pipe, so `OutReader` reads on its own thread and the turn ends
  with the process once the reader got through what the CLI wrote (it ended, or
  sat 500 ms with nothing, 30 s at most). An event is checked under the lock
  against the turn being over.
- A turn about to start takes the helper's run under the same lock that found
  it free (`claim`), so text arriving before the process is up queues. A stop
  that finds a turn claimed but with no process yet marks it `stopped`. A turn
  that cannot start counts as an ended turn with its error; text queued behind
  it fails with it.
- On Unix each turn's process leads its own process group.
- `send` mid-turn to a Claude Code helper joins that turn: stdin stays open
  until the turn's first result and the text goes in at the next tool boundary;
  it counts as delivered when the CLI says it took it in. If the CLI goes
  first, the text starts the next turn; if that fails, it returns to its sender.
  Codex's `exec` reads stdin once, so text waits for the next turn.
- `send` with `interrupt` puts the text behind the queue, kills the turn's
  process tree, and the reader's end of turn starts the next (`background::interrupt`);
  a turn that will not stop within 3 s leaves the result `queued`.
- A `claude -p` with a `run_in_background` task open waits for it and takes its
  end up in the same run, so the helper reads `working` and its caller gets one
  notice.

## Asks

A Claude Code helper always gets Luna's MCP server, if only for
`--permission-prompt-tool mcp__luna__luna_permission` (hidden from its model by
`--disallowedTools`). Permission prompts and `AskUserQuestion` arrive as that
call, held on a Condvar until `luna_session_answer` (allow / deny, or `answers` by
question text) or the turn's end. Several at once queue; the oldest is shown.
Each has an opaque `askId`: one no longer open is refused, one never had
(`Registry::asked`) is unknown, and without an id an answer goes through only
with a single ask held.

Codex's `exec` asks nobody (approval `never`) and puts a question in its reply,
unless spawned with `approval: auto-review` (`--approve-for-me`), where a
reviewer model answers.

## Leftovers

Each helper has one job for its life; every turn's process joins it. Between
turns `children_busy` is what the turn left (`procs::left_in`), each process kept
with when it last worked (`Leftover`); `luna_session_list` shows them as `leftRunning`.
A stop between turns ends only them and holds the helper (`Entry::ending_left`),
so a send queues rather than start a CLI in a job being emptied. A turn whose
CLI went before its model did anything (`Observed::acted` false: an unknown
`--agent`, a missing model) first ends what descends from that CLI
(`procs::end_tree_of`), sparing what earlier turns left. The CLI's resident
helpers (`codex-code-mode-host` and its `conhost`) are furniture while they do
not move (`without_furniture`). See [power and lifecycle](power-and-lifecycle.md)
for the activity rule.

## The window's view

The window gets the list as `agent://background` and lists a chat's helpers on
its row's chip.

- **Watch** reads the helper's conversation off its transcript
  (`background_transcript`, by the same readers `luna_message_list` uses, tool calls
  marked) from a cursor, and again every second while a turn runs, every five
  between turns (`useHelperTranscript`). The file is the helper's own, from its
  record, never one the window names. A look reads at most 2 MB back from the
  file's end into memory (`VIEW_SCAN`), from a line's start, passes over a line
  longer than 256 KB (`VIEW_LINE`), and parses the rest with the readers that
  work on bytes (`claude::session::messages_in`, `codex::session::messages_in`);
  it keeps the newest 200 messages, each cut at 20,000 characters, and says
  when it passed anything over (`passedOver`). Read-only: nothing it shows takes
  input.
- **Delete** (`background_delete`) refuses a helper at work unless
  `stopWorking`, and keeps a worktree with unsaved work unless `dropUnsaved`
  (agents/teardown.rs `delete_by_user`). A chat's Delete dialog asks first what
  each of its helpers has at stake (`background_at_stake`: its turn, its
  worktree and the work in it) and offers each as a box of its own.

## Opened as a chat: handed over

Between turns, with nothing it left running, the person can open a helper as a
chat of their own, in two steps so a failure leaves the helper its parent's:
`background_promote` marks it `promoting` (no turn starts, sends queue, its
parent's kill and delete are refused) and returns the row's data (same id,
session id to resume, no parent); the window answers `background_promoted`. On
success `agents::take_over` changes its hands under one registry hold, then
takes its record with what was queued for it, and tells its parent and every
other sender once ([agents](agents.md#rights)); on an error it is unmarked and
goes on. The window seats the row in a pane only once the core has let go, and
`ensure_*_session` refuses a helper's id until then. It tells the core again on a
failure (`SETTLE_RETRY_MS`, up to about two minutes), and a window coming up
settles what a gone one was opening. `promoting` and since when are saved in
`helpers.json`, so a core restarted meanwhile holds the helper the same way. An
opening unsettled for 15 minutes (`PROMOTE_TIMEOUT_MS`, checked by the activity
sampler) goes back to the parent, which hears so (`handover::opening_lapsed`);
a row made for it after that is refused (`background_promoted` answers false)
and the window lets go of it.

## Worktrees

A spawn with `worktree: true` waits up to 20 s for the CLI to say where
(Claude Code's `init` event, Codex's rollout) and returns `worktree` and
`branch`, or `worktreePending`. `background::working_dirs` gives the folder it
was sent to and the worktree it made (its first `init`'s cwd; a later one says
where the shell went, `home_of` maps it back), between turns too, for
`worktree_shared`, `live_cwds` and `account_in_use`.

## Outliving the core

### Background helpers: `helpers.json`

Every 2 s, as the core leaves (`stop_all`), and at once on what a killed core
must not undo (a spawn, a conversation named, a delete, a question, a
handover), the registry is written to `<data>/helpers.json` when changed, one
save at a time. It holds the spec without its token, the session id, the
worktree, the turn count, queued text with each sender (`Queued`), and the
agent ties (`agents::Ties`: parent, name, settings, debts, counts shown); the
conversation itself stays in the CLI's transcript ([what Luna keeps of
conversations](#what-luna-keeps-of-conversations)). A `send` that queues writes
at once. A batch a turn claimed is
saved as `Entry::in_flight` until stdin has it.

The next core reads it before the hub listens (`background::restore`,
`agents::tie_back`, fresh tokens). A turn that ran then counts as ended, and the
helper is `cut_short` until its next turn. Nothing is written before `restore`
has read the file, so an empty registry never overwrites it.

- A debt comes back ahead of the chat it is owed to: a debt for an unknown
  caller is dropped only `CALLER_RETURN_MS` (10 min) into the core's life
  (`owed_stands`).
- Text queued behind a stopped turn stays, never run; only its sender hears of
  it (`Entry::undelivered`, `undelivered_text`), and `read` / `wait` name it as
  `undelivered`. `luna_session_stop` drops the queue.
- A helper stopped before any turn named its conversation keeps its opening
  (`Entry::opening`); the notice says it never started (`never_started`).
- **`resumeOnRestart`** (in `settings`, so in the ties): `resume_stopped`, once
  the hub listens, starts a turn on each `cut_short` helper with a tagged line
  and the undelivered text; debts on it move past the counted turns
  (`resumed`), and the caller gets one "cut short, resumed" notice
  (`Settled::Resumed`). It runs only where nothing of the old turn can overlap
  (`Entry::resumable`: the old core stopped it itself, or its processes were in
  a job tied to the core, `Entry::tied`; off Windows only if `end_last_ties`
  finds nothing left). Otherwise the helper is `Entry::unsure` and the caller
  decides.
- A `budgetTokens` line is marked in the notice itself (`Queued::budget`), so
  `stdin_took` is the one receipt (`budgets_landed`).

### The person's chats: `chat-ties.json`

`agents::save_chats` keeps, for each chat of the person's that an agent is owed
an answer by or holds text for, its `Ties`, turn count and transcript, and the
sends Luna holds (`Held`); and the lines still to be told (`Registry::news`,
such as a parent's "the person took over your helper"), so one is said once
whatever comes between. `restore_chats` reads it; a chat is `Known::waiting`
until the window starts it and `register` clears the mark. The count goes on
(`activity::keep_count`). A debt the count had not passed is one whose turn went
too (`Owed::cut_short`). A chat not back within `SESSION_RETURN_MS` (1 min) is
reported as not started again. A chat is never anyone's helper: one an earlier
Luna saved with a parent (a visible helper) comes back with none, and none of a
helper's settings. `save_chats` stops while sessions are being stopped
(`freeze_chats`), so the next core does not read what quitting did to them.

Neither CLI continues a turn a core killed (Claude Code's `--resume` needs an
environment variable Luna does not set; Codex's continuation lives in the daemon
`--no-daemon` keeps out), so the window restarts chats between turns, and the
person carries them on.

### Text a core went in the middle of typing

A held send is marked on disk as it goes in (`Held::going_in`: first words and
the transcript's end). The next core checks the transcript (`reconcile_held`):
found, it is not typed again and the sender is owed a cut-short turn; not
found, it goes in as usual; no transcript, it is not typed and the notice says
Luna cannot tell (`Owed::unsure`). A debt told "NOT done yet" comes back so on a
background helper (`Ties::told_pending`): on Windows what it had running went
with the core, so its notice says the core restarted.

### A spawn under way

`spawn` writes a `SpawnTaken` (`SpawnUnderWay`) to `chat-ties.json` before
anything is made. `spawn_is` names the helper as it registers. Left in the
file, it means the core went mid-way: `take_up_spawns` owes the caller from the
saved helper (`helpers.json` is read first); `settle_spawns` gives up the rest
(`settled`), after which `spawn_is` refuses a late helper. With
`resumeOnRestart` `spawn_afresh` asks again. Otherwise the caller is told it
never completed, prompt in a file (`never_completed`). A worktree new since the
spawn with no session in it is reported as `worktreeLeft`, never removed.

### A worktree half-made

Luna names every Claude Code helper's worktree (`helper_worktree_name`, kept in
its `Spec`) and passes `--worktree=<name>` (`worktree::name_ok`,
`worktree_arg`). A fresh start goes back only into that one and only if it is
`as_made`: on its branch, HEAD at the commit the branch was created at (oldest
reflog line), and nothing in it that is not in that commit except the
`.claude/settings.local.json` the CLI copies in. Anything else is reported once
as `worktreeLeft` (`worktrees_left_line`) and the helper takes the next name
(`name_to_go_on`). Codex takes no name, so a Codex helper starting afresh makes
a new worktree, and what is new since the spawn is named (`codex_new_since`,
`report_codex_worktrees`, by `Registry::worktrees_before`).

## Invariants

- No turn starts after `stop_all`; `stop_all_sessions` before an update stops
  turns too (`stop_turns`), or the old core could not be replaced.
- A core that dies takes each helper's whole job with it
  ([power and lifecycle](power-and-lifecycle.md#process-jobs)).
- Luna never copies a transcript; the text it does keep is listed below, each
  with when it goes.

## What Luna keeps of conversations

Luna does not mirror a CLI's transcript, but it does keep text of its own:

| Where | What text | Kept until |
| --- | --- | --- |
| `<data>/helpers.json` (`background/persist.rs`) | a helper's opening prompt (while no turn has named its conversation), and text queued, in flight or undelivered, word for word with its sender | the text is delivered or handed back |
| `<data>/chat-ties.json` (`agents/persist.rs`) | sends Luna holds for a draft (`Held`), a spawn under way with its request and prompt (`SpawnTaken`), lines still to be told (`news`) | the send goes in / the spawn settles / the line is told |
| `<data>/agents.jsonl` (`agentlog.rs`) | the first 300 characters of each spawn's prompt; on its end the last message, up to 3,000 (`history` shows 400) | cut to the newest half past 2,000 lines |
| `<data>/agents/<id>-<hash>.md` (`agents/said.rs`, `agents/delivery.rs`) | a notice's or cut `wait`'s full answer (every message since the question), long prompts and messages sent as files, undelivered text named to its sender | swept after a week |

What is never kept: the CLIs' own transcripts and rollouts (Luna reads them in
place), and anything in `luna.log`, which names sessions but holds no chat text.

## Failure modes

| Failure | Behavior |
| --- | --- |
| Core killed mid-turn | turn counts as ended; caller told "did not finish"; optional auto-resume |
| Core killed between claim and stdin | `in_flight` neither forgotten nor replayed |
| Disk full | letting go of a tied job is one unlink (`tied-jobs/`), which a full disk does not refuse |
| Window gone while opening one as a chat | the next window settles it by whether the row exists |