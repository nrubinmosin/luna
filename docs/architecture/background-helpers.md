# Background helpers

What `luna_session_spawn` makes, every time: no chat row, no pty, all in the core, so no
window is needed. The person watches one read-only or opens it as a chat of
their own, which hands it over for good. Also: how helpers, and what agents are
owed by the person's chats, outlive a core restart.

## Responsibility

- Run each turn as one headless CLI process and parse its event stream.
- Queue, join and reserve text sent to a helper; hold the permission prompts
  and questions its CLI asks for `luna_session_answer`, and answer them where
  the CLI asked.
- Stop a turn by the CLI's own interrupt where it has one, by a kill where
  not.
- Track what its turns leave running, and end it with the helper.
- Persist helpers and agent ties so the next core can take them up.

## Key modules

| Path | What it holds |
| --- | --- |
| `background/mod.rs` | the helpers' registry |
| `background/types.rs` | what a helper is made of (`Spec`, `Entry`, `Observed`) |
| `background/queue.rs` | text queued, joined and reserved; command text re-checked against its sender |
| `background/places.rs` | the places a line has for helpers at work: the check every turn passes as it is claimed, spawns' promised places, turns waiting for one (`wake`) |
| `background/command.rs` | a turn's argv (golden-tested) |
| `background/run.rs` | a turn's process, `OutReader` |
| `background/appserver.rs` | a Codex turn's JSON-RPC with its `codex app-server`: the thread and turn requests, steers, interrupts, the server's asks |
| `background/events.rs` | Claude Code's JSON stream and a turn's end |
| `background/control.rs` | kill, interrupt (the CLI asked first, `ask_to_stop`), leftovers |
| `background/ask.rs` | log as screen, transcript, held asks and their answers on stdin |
| `background/rows.rs` | `agent://background` rows, the read-only view (`background_transcript`), the person's delete (`background_delete`, `background_at_stake`), opening one as a chat |
| `background/persist.rs` | `helpers.json` across cores |
| `agents/persist.rs`, `agents/held.rs` | chat ties, text held for the person's chats |
| `agents/handover.rs` | a helper opened as a chat, handed over to the person |

## A turn

- One process per turn: `claude -p --output-format stream-json --verbose`,
  resumed by the session id the first turn printed, prompt on stdin (no
  command-line limit); or `codex app-server`
  ([ADR-0032](../adr/0032-codex-helpers-on-app-server.md)), spoken to in
  JSON-RPC on its stdio (`appserver.rs`): `initialize`, then `thread/start`
  (the folder as `cwd`, the model, the role layers as
  `developerInstructions`) or `thread/resume` of the thread an earlier turn
  started (the same), then `turn/start` with the prompt and the effort. At
  `turn/completed` Luna closes its stdin and the server goes (`see_out` ends
  it, and only it, if it is still there 10 s on). The conversation lands in
  the CLI's own transcript or rollout.
- A Codex thread is the helper's conversation (`Entry::session`) only once
  `turn/start` is answered: Codex writes a thread at its first turn, and
  resuming one never given a turn says "no rollout found" (0.169.0, seen
  live). Until then the opening prompt stays saved as what the helper is to
  start on, whatever stops it (a core's death, a stop, a refused
  `turn/start`). A `thread/resume` refused so, or answered with another
  thread's id, starts a new thread with the turn's text, once.
- The approval and sandbox Luna passes (named by the parent, or its closed
  fallback) go typed in `thread/start` and `thread/resume` as well as `-c`:
  a resumed thread's saved settings win over `-c`, not over these. The
  server's answer is checked against them (`settings_differ`); a thread that
  answers with another approval, reviewer or sandbox does not start its turn
  (fail closed), its error saying which. Luna's own profiles (read-only-net,
  a core deny) are defined by `-c` and named typed as `permissions`, which
  the answer's `activePermissionProfile` is checked against (0.169.0, seen
  live, a new name on every start and resume alike).
- Each message goes with a `clientUserMessageId`: `turn/start` the id of its
  batch's first text (`Queued::line`), a steer its text's own. Its text is
  kept until the server has it, a Codex send that starts a turn included
  (`in_flight`), and saved as sent (`Queued::sent`) before `turn/start`
  goes (`send_turn`); a refused `turn/start` hands it back. The rollout keeps
  the id as the `client_id` of the message's `UserMessage` item (0.169.0,
  seen live), so a core taking the helper up settles each text sent with no
  word back by its thread alone (`settle_sent`, `stopped_lines`): there, the
  model's; not there, or no rollout, handed back to its sender as one that
  may not have arrived (`maybe_reached`). Luna never sends it again by
  itself; what was only queued goes on as ever. A first turn's opening with
  no thread saved for it is handed back the same way, not started again in a
  new thread: the server may have made one Luna never heard of (an orphan
  thread, its residual).
- A turn whose server breaks the protocol (a request of Luna's refused, a
  thread or turn with no id, a turn that never completes while Luna did not
  stop it) ends failed and counts as a failed session of its version for
  the CLI canary, whatever it exits with (`went_wrong`). A server that does
  not answer `turn/start` within 3 minutes is ended failed the same way
  (its canary watch held until `turn/start` is answered, so it counts
  however late, `session_awaits_start`):
  asked to stop, then killed (`watch`, `stalled`). A turn that has started is
  never ended by the clock, however long it is quiet (one long command can
  be): its parent hears of the silence as of any helper's, "written nothing
  for N min" once the turn has said nothing for 30 minutes (`QUIET_TURN_MS`,
  off `turn_quiet_ms`, which every message of the server moves), and decides
  whether to interrupt or delete it.
- Busy is the process running, not stdout open: a job the turn left may have
  inherited the pipe, so `OutReader` reads on its own thread and the turn ends
  with the process once the reader got through what the CLI wrote (it ended, or
  sat 500 ms with nothing, 30 s at most). An event is checked under the lock
  against the turn being over.
- A turn about to start takes the helper's run under the same lock that found
  it free (`claim`), so text arriving before the process is up queues. In that
  hold it passes its line's places ([agents: places](agents.md#spawning),
  `places.rs`): with none free its text stays queued and it waits, in order,
  for one (`waits_since`, `wake`). A stop
  that finds a turn claimed but with no process yet marks it `stopped`. A turn
  that cannot start counts as an ended turn with its error; text queued behind
  it fails with it.
- On Unix each turn's process leads its own process group.
- `send` mid-turn to a Claude Code helper joins that turn: stdin stays open
  until the turn's first result and the text goes in at the next tool boundary;
  it counts as delivered when the CLI says it took it in. If the CLI goes
  first, the text starts the next turn; if that fails, it returns to its sender.
  To a Codex helper it steers the turn (`turn/steer`, `queue.rs` `write_line`)
  once `turn/start` has answered with the turn's id, and counts as delivered
  once it is a `userMessage` item of the turn (its `clientId`), not when the
  server answers the steer: an interrupt before the model reads it drops it
  (Codex clears its pending input), and it goes back to its sender as
  undelivered. One the turn does not take ("no active turn to steer": it
  ended first) starts the next turn the same way. Before
  the turn has an id, while it is being stopped, behind queued text, and for
  a notice, text queues.
- `send` with `interrupt` puts the text behind the queue and stops the turn,
  and the reader's end of turn starts the next (`background::interrupt`); a
  turn with no process yet after 3 s, or not over 15 s after it was stopped,
  leaves the result `queued`. `luna_session_stop` stops a turn the same way,
  dropping the queue. A Claude Code turn whose model is at work, its stdin
  still open, is asked first (control.rs `ask_to_stop`): Luna writes the CLI's
  own `interrupt` control request on its stdin and closes stdin behind it, and
  the CLI stops its tools as Esc does, writes the turn's `result` and goes
  (2.1.325: the `result` at once, the exit about 2 s later). The conversation
  is the CLI's to keep, and the next turn resumes it. Luna gives the `result`
  5 s and the exit 5 s more (`ASKED_RESULT_WAIT`, `ASKED_EXIT_WAIT`), then
  kills the turn's process tree (`end_turn_process`), as it does at once for
  one past its `result` (what keeps it up then is background
  work, which an interrupt does not end) or one whose stdin has closed; what
  the turn left in the background goes with that kill. A stop's wait runs on
  a thread of its own; an interrupt's is part of its own wait. The `result` of
  a turn so asked is the stop's, not a failure: its record's outcome is
  `stopped` (`STOPPED_BY_STOP`, `INTERRUPTED` its reason), never `failed` nor
  finished, `luna_session_wait` gives the reason as `stopped`, not `failed`,
  and the notice says the turn did not finish, with it, whether the CLI
  stopped it or the kill did. The reason outlives the start of the next turn
  (`Observed::stopped`, control.rs `stop_of`), which a send with `interrupt`
  starts at once, until a later turn ends; an end the stream wrote before the
  stop, read after it, does not overwrite it, and a turn stopped before its
  process started ends `stopped` too. The canary (cli/canary.rs) counts an
  interrupt's exit as Luna's doing, as a stop's.
  A Codex turn is asked by `turn/interrupt` (appserver.rs
  `ask_server_to_stop`), its stdin left open for its `turn/completed`, and
  its tree killed only if no `turn/completed` comes within 5 s
  (`INTERRUPT_GRACE`) and it has not completed either (a server that lingers
  past `turn/completed` is `see_out`'s to end, alone). The items an
  interrupted Codex turn leaves open (0.169.0 sends no `item/completed` for
  them) are closed in its log. 0.169.0 leaves the interrupted command running
  until its server goes (seen live), so once the turn is over Luna ends what
  the turn started and left (`end_stopped_turn`): on Windows what in the
  helper's job descends from the server and was born in the turn, elsewhere
  what the keeper's listing of the job (its groups, sessions and mark) holds
  that earlier turns had not left.
- A `claude -p` with a `run_in_background` task or a background subagent open
  waits for it and takes its end up in the same run, so the helper reads
  `working` and its caller gets one notice. Its stdin stays open past the
  turn's `result` while a background subagent or workflow of it runs, for at
  most 3 hours (`LISTEN_MS`, below), and with its stdin closed it waits only `CLAUDE_CODE_PRINT_BG_WAIT_CEILING_MS`
  (10 min by default, 2.1.319+) and then stops that work, subagents' unfinished
  and uncommitted edits with it; a helper's turn sets it to 3 hours
  (`background/command.rs` `BG_WAIT_CEILING_MS`, the
  `helper.background-wait` item): subagents and gates run long, but `0` would let a
  forgotten watcher (a dev server, `tail -f`, a Monitor with no end) hold the
  turn, and every message to the helper, for good. Until then the turn is
  busy; once its result is in (`Run::after_result`) and its stream has been
  quiet for 30 min, the caller is told it waits on its background work (what
  its job does is not measured), by the calls its transcript has open
  (`claude::session::background_calls`)
  and the processes in its job (`waits_on_background`), and `luna_session_stop`
  ends it; with a subagent among them the line does not call it a hang. The
  stream does show a subagent's steps (2.1.322 writes a `task_progress` event
  for each tool call of one) but not one long command of its, which the job
  does. When the CLI stops that work, its stderr line ("Background tasks still
  running … stopping them") is caught (`run.rs` `stops_background`) and kept
  as a loss of that turn (`Observed::losses`, persist.rs `record_loss`: the
  turn it ended, its cause, the subagents it took, the callers owed an
  answer as it came, taken then: a message one of them queues behind that
  turn moves the turn its debt counts from, not who it is), saved across
  cores with those told so far and kept across later turns (a queued
  message starts one at once). A later loss is one of its own, with its own
  cause. Every line that settles the debt of one of them says each loss it
  has yet to hear of, by its own cause, to that caller (rather than "it has
  said nothing"), the "seems stuck" line of a woken lead included, as does
  a wait, read or report that hands the answer over (`backgroundStopped`,
  owed.rs `background_stopped_note`). A delivered notice marks that caller
  told of each loss whose line it carried (notifier `ack_background_stop`,
  lines.rs `loss_line`; never a line without one, such as an exited
  helper's); a hand-over, of the losses its reply said as it was put
  together (`hand_over_said`), so one recorded while a `wait_many` waited on
  others is still to be said. A reply that never reached its caller unmarks
  that caller alone, and only for those losses; a loss is done once every
  caller still owed has been told, and goes when the next is recorded. What
  is saved is no stop to 0.19, which reads none of `losses` and fails all of
  `helpers.json` on a `stopCause` it does not know; a stop 0.19 saved comes
  back as a loss with no names (`take_legacy_stop`), and a cause a later
  Luna adds reads as `ceiling`. The quiet notice of a turn that has
  answered and waits on its background work is told once per turn apart
  from the plain quiet one (`Owed::told_background_wait`).
- A Claude Code helper's background subagents live inside its turn's
  process, and go with it. From the turn's stream Luna keeps the
  subagents the process started (`Run::subagents`, events.rs
  `track_subagent`), one entry per run: a `task_started` of `task_type`
  `local_agent` not said to run in the foreground (`is_backgrounded`
  false; a start that does not say counts), or a `task_id` not seen yet
  that a `background_tasks_changed` lists (one moved to the background
  since), by its `task_id`, `run_id` and `description`. Only top-level
  ones: a subagent's own subagents (`spawn_depth` past 1, a
  `parent_task_id`) live inside it and go with it, so their parent is the
  one named. A resumed subagent keeps its `task_id` under a new `run_id`:
  its start is a run of its own, and a listing never touches a run already
  kept, so one late from an earlier run stands for nothing. Only a run's
  own `task_notification` ends it, never its absence from a listing, whose
  order against the notifications the CLI does not fix; one that names no
  run ends the one run of that `task_id` still open, and none while two
  are. `completed` or `failed` is done, as is `stopped` while the main
  thread is at work (the model's `TaskStop`); `stopped` with nothing from
  the model since the turn's `result` (`Run::model_done`) is the CLI
  stopping it on its way out, and marks it cut for good (`Subagent::cut`).
  Any other status, or none, ends nothing. The stderr line above marks
  nothing by itself: the exit thread reads all of stdout before the turn
  is over (`turn_over`), so a subagent's end still in that buffer as the
  line came counts. When the process exits with any cut or not done
  (killed, gone on an error, or stopped at the ceiling), `turn_over`
  records them in the turn's loss, named by description, once per
  `task_id`, with the callers owed an answer then; the cause stays
  `Ceiling` when the stderr line came in that turn, and otherwise says how
  the process went (`Loss::by`: `exited`, or `stopped` when Luna stopped
  it: a stop, an interrupt, its own going, `Run::luna_stopped`). The line
  its callers get names them, and never says they finished (lines.rs
  `subagents_lost`): "2 background subagents ("Implement slice A",
  "Implement slice B") ended with its turn without finishing; their work
  is not done", with the cause for the ceiling, and that a send has it
  check what they left and carry on, with Luna helpers for work that must
  outlive its turn; after Luna's own stop it says only that. It also keeps
  a lead's "all delegates done" wake from standing in for the notice.
- A core that leaves (`stop_all`) gives each turn it stopped up to 2 s for
  its exit thread to record that end (`keep_unrecorded_ends`); one still
  unrecorded keeps its Claude Code subagents known not done as lost to
  Luna's stop, at the turn the next core counts, whose cut-short notice
  says them. What that cannot cover: a subagent whose start was still
  unread in stdout, and a Codex turn's subagents, read from its rollouts
  only at `turn_over`; the next core says only that the turn was cut short.
- A Claude Code lead whose CLI stays up past its result for its own
  background tasks hears of its helpers meanwhile (issue #73). At a `result`
  with tasks running (the last `background_tasks_changed` event lists some),
  a lead's stdin stays open (`events.rs` `keeps_listening`,
  `Run::listening_since`), and the notifier counts it as ready for a notice
  between its CLI's turns (`takes_notices`). The notice joins on stdin
  (`Placed::Joined`, as a `send` mid-turn does), and the CLI takes it at once
  and answers it in a turn of its own with its own `result`, the background
  work running on (2.1.325). That is no Luna turn: the process is one turn,
  counted at its exit as ever, so the notice marks no turn as one on a
  notice (`notice_turn`), and the lead's parent hears of the turn as it would
  of any other. A notice that finds the lead mid-turn at the look waits for
  the next look. The CLI's own turn on a task's end counts as one from its
  `task_notification`, ahead of that turn's first `assistant`, and while a
  line written to stdin is still to be taken in (`Entry::fed`) the lead
  takes no other notice: a `result` before then is of a turn the CLI started
  by itself. A notice that the look found taking notices but that finds the
  CLI at a turn by the time it goes in (`send_as` checks `listening` again)
  is queued, and the next `result` that finds the lead taking notices moves
  the queue onto stdin in order (`feed_queued`), up to text a CLI would read
  as a command, which waits for the next process and its sender's reach
  asked again (`settle_batch`); what is sent meanwhile queues behind it, not
  past it. Such a notice marks no turn either (`send_marked` says it was
  queued for that stdin). A send moved so is answered within this process,
  not the next one its debt was put past as it queued: the move is kept
  with that process's count (`Entry::drained`) and the debt brought back to
  this process (`agents/owed.rs` `moved_onto_stdin`), by the event's reader
  under the registry's lock or, when the move came between the text queuing
  and its debt going in, by the send itself (`send_tracked` gives it the
  text's id). The last `DRAINED_KEPT` moves outlive their process, so a send
  whose text was taken in and answered, and the process gone, before its
  debt went in still finds it; a line the process never took in loses its
  move as it goes (`sort_out_untaken`). Not while more of that sender's text waits
  for a later process (queued, or with that one gone, started on or fed to
  the next), which answers it. A wait going by the count the send returned goes by the
  debt's (`Owed::moved_from`), and a line that goes untaken with the process
  is put past it again (`retried`). With stdin open the CLI never goes by itself, and
  its own ceiling does not count, so Luna closes stdin once the last task
  ends (`background_tasks_changed` with none), at a `result` with none left,
  when nothing is owed to the lead any more between its CLI's turns (the
  notifier's look, `listen_only_while_owed`, under the registry's lock so
  that a debt made meanwhile is not missed), or after 3 hours (`LISTEN_MS`, as long as
  the CLI's own ceiling, which counts from there). None of these but the
  3 hours closes it while a line written to it is still to be taken in, a
  `result` included (`holds_untaken`): that `result` is of a turn the CLI
  started by itself, and the line's own turn's `result` is the one judged.
  What it was given before then it still runs, then goes; what it never took
  starts the next turn, as for any joined text (`sort_out_untaken`), and a
  core gone meanwhile settles it against the transcript like any fed line
  (persist.rs `settle_lines`). Every helper's stdin also stays open past a
  `result` while a background task that may ask runs, for its asks
  ([Asks](#asks)): a subagent or a workflow (`task_type` `local_agent`,
  `local_workflow`, or a task that says no type; `Run::asking_tasks`, events.rs
  `may_ask`), not a shell or a monitor, which ask nothing and which the CLI's
  own wind-down ends with stdin closed, as before. What counts is the latest
  `background_tasks_changed` listing: one that names no such task closes
  stdin, unless the turn the CLI takes the end up in has already begun (its
  model has written since the `result`, `Run::model_done`), which may ask too:
  then that turn's `result` closes it. A `task_notification` alone keeps
  nothing open, as the CLI may take no turn on it. Residual: a turn the CLI
  begins after the listing that names none finds stdin closed, and an ask in
  it fails at once ("Stream closed"), which its model reads as a refusal. A lead
  owed nothing keeps it while one runs too, and a helper without Luna's tools
  takes no notices and is never closed for "nothing owed". None of the closes
  but `LISTEN_MS` comes while an ask is held on that stdin: one closed under
  it fails the ask (2.1.325: "Tool permission request failed: AbortError:
  Stream closed"), and `LISTEN_MS`'s close lets the held ask go with it. So a
  forgotten subagent holds a helper's turn up to 3 hours with stdin open,
  then the CLI's own ceiling, 3 hours more, as it holds a lead's; a forgotten
  shell, only the CLI's own wind-down.
- A Codex helper's server has no such ceiling: its turn ends at
  `turn/completed`, and the server goes as Luna closes its stdin. Its
  subagents run in that process and go with it. At a Codex helper's turn
  end, before it counts, Luna reads the rollouts of
  the subagents begun in the turn, and one whose last turn, begun since
  the parent's, aborted `interrupted` with the parent's end (codex::session
  `subagents_cut_at_turn_end`) is recorded as a loss as above
  (`StopCause::TurnEnd`), with the callers owed an answer then. A server
  killed or crashed takes those still at
  work with it, its own turn's end written or not: a subagent whose turn,
  begun since the parent's by a timestamp that reads, is still open counts
  the same. A thread with no turn begun since then cuts nothing, and each
  rollout's header is read once. A subagent's own subagent cut counts as
  the top-level one it is under, once, as Claude Code's do. Each is named
  as its rollout's `session_meta` names it (`agent_nickname`, else
  `agent_role`; none, ""), and the callers hear the Claude Code line's
  words, "N subagents (…) ended with its turn without finishing; their
  work is not done", with a send to have it wait for any subagent it
  spawns before it ends its turn, or, Luna having stopped it, that alone
  (lines.rs `subagents_lost`; `SUBAGENTS_INTERRUPTED` for a stop 0.19
  saved uncounted).

## Asks

A Claude Code helper's turn runs with `--permission-prompt-tool stdio`, tools
or not. Its permission prompts and `AskUserQuestion` arrive on its stdout as
control requests (`{"type":"control_request","request_id","request":
{"subtype":"can_use_tool","tool_name","input","tool_use_id",...}}`,
events.rs `permission_request`). The reader of the process running takes
each one under the registry's lock (ask.rs `take_ask`), so an ask belongs to
that process's turn and to no later one, and holds it in the helper's record
with where its answer goes (`Held::reply`): no thread waits on it. It is held
until `luna_session_answer` (allow / deny, or `answers` by question text), the
turn's end, or 30 minutes (`ASK_TIMEOUT`, checked at each read of the records,
`expire_asks`, then a deny). `answer` writes the answer then and there on the
stdin of the process that asked, as the control response the CLI takes
(`{"type":"control_response","response":{"subtype":"success",
"request_id","response":{"behavior":"allow","updatedInput"} | {"behavior":
"deny","message"}}}`), one escaped JSON line, and the ask goes. Refused at
once, never shown to the caller: an ask read with that stdin closed (the CLI
has failed it; nothing is written), the same request id again, one about a
tool call this turn's stream has not shown (`Run::tool_uses`, the newest
1,024 ids: the CLI writes the call's message first; a deny is written), and
one past `ASKS_PER_TURN` (8) held at once (a deny). The tool-call check is on
order, not on who wrote the line: a process that can write to the stdout can
write both lines. It applies only to an ask that names a call: one naming
none, and the sandbox's network ask (`SandboxNetworkAccess`, under an id the
CLI makes up for it, `MACHINERY_ASKS`), are held as they are. An ask the CLI
withdraws (`{"type":"control_cancel_request","request_id"}`, when its own
wait ends) goes at once, its place freed; a late answer to it is refused. An ask whose stdin closes while it is held, or whose process
goes, is let go (`drop_dead_asks`), and an answer to it is refused. Its stdin
is kept open for them while it may ask: through the turn, and past its
`result` while a background subagent or workflow runs ([A turn](#a-turn)). The CLI's other requests on stdin (an MCP server's
elicitation) are refused at once, as nobody is there to answer them. Luna's
MCP server is a helper's only with Luna's tools (`tools`), as for Codex: a
helper without them has no token and no server. Several asks at once queue;
the oldest is shown.
Each has an opaque `askId`: one no longer open is refused, one never had
(`Registry::asked`, which forgets a helper's with its turn's end) is unknown, and without an id an answer goes through only
with a single ask held. A permission's `detail` is kept whole up to 8 KB and
marked where cut, and its `source` says whether the helper itself or one of
its subagents made the call: the call names only its `tool_use_id`, and the
helper's latest 64 tool calls are kept with who made them, as their stream
messages say (`Entry::calls`), an ask held before its message comes in being
named when it does (agents.md, "State and asks").

A Codex helper's server asks as JSON-RPC requests of its own (`appserver.rs`
`server_request`), held the same way: `item/commandExecution/requestApproval`
(the command), `item/fileChange/requestApproval` (the files of the item it
names), `item/permissions/requestApproval` and 0.169.0's v1
`execCommandApproval` / `applyPatchApproval` as permissions,
`item/tool/requestUserInput` as a question. A thread of its own waits for the
answer (`reply_when_answered`) and replies on the turn's stdin: `accept` or
`decline` (v1: `approved` / `denied`; a permissions grant is the profile asked
for, for the turn, or none), a question's answers by its id, in the order
asked (one with options and no `isOther` takes only those; an `isSecret`
answer goes into the reply alone, written nowhere else); unanswered at
ASK_TIMEOUT, a refusal; the turn gone, nothing. A request the server settles
itself (`serverRequest/resolved`: an interrupt, its own timeout) closes its
ask at once, and a late answer to it is refused, saying so. An MCP server's elicitation is
declined and any other request (a dynamic tool, a token refresh) refused at
once, as a Claude Code helper refuses its CLI's own requests. What it asks
follows its approval: the one its parent named (`never`, `on-request`), or,
none named, its config's (closed `on-request` where Luna cannot read it); with
`approval: auto-review` a reviewer model answers instead (the three settings
`--approve-for-me` stands for, as `-c`, `AUTO_REVIEW_OVERRIDES`). Helpers do
not run in plan mode, the only one where 0.169.0's model has its question
tool, so its questions come only should Codex give the tool elsewhere.

## Leftovers

Each helper has one job for its life; every turn's process joins it. Between
turns what the turn left (`procs::left_in`, each process by name, pid and start,
`Leftover`) is the helper's record's shells (`life_of`): active, holding the PC
awake and a power-off back, for as long as it is there, however long it does
nothing; `luna_session_get` shows it as `leftRunning`, and autoDelete waits for it.
A stop between turns ends only them and holds the helper (`Entry::ending_left`),
so a send queues rather than start a CLI in a job being emptied. A turn whose
CLI went before its model did anything (`Observed::acted` false: an unknown
`--agent`, a missing model) first ends what descends from that CLI
(`procs::end_tree_of`), sparing what earlier turns left. The CLI's resident
helpers (`codex-code-mode-host` and its `conhost`) are furniture, never counted.
See [power and lifecycle](power-and-lifecycle.md#the-lifecycle-record) for the
record.

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
  `stopWorking` (agents/teardown.rs `delete_by_user`); a worktree it ran in
  stays as it is. A chat's Delete dialog asks first what each of its helpers
  has at stake (`background_at_stake`: its turn) and offers each as a box of
  its own. A delete from a chat's dialog names the chat (`ofChat`). Under
  the delete's hold, Luna checks that the helper still belongs to that chat
  or has been left with no chat above it.

## Opened as a chat: handed over

Between turns, with nothing it left running, the person can open a helper as a
chat of their own, in two steps so a failure leaves the helper its parent's:
`background_promote` marks it `promoting` (no turn starts, sends queue, its
parent's kill and delete are refused) and returns the row's data (same id,
session id to resume, no parent); the window answers `background_promoted`. On
success `agents::take_over` changes its hands under one registry hold, then
takes its record with what was queued for it, and tells its parent and every
other sender once ([agents](agents.md#rights)); on an error it is unmarked and
goes on, unless it is being handed over on an earlier word that the row was made
(`handing`), which stands: undone then, its next turn would start beside the
chat. The window seats the row in a pane only once the core has let go, and
`ensure_*_session` refuses a helper's id until then. It tells the core again on a
failure (the window's `dialogs/helpers.rs` `SETTLE_RETRY`, up to about two
minutes; an update's last write of the list waits for the first 7.5 s of them),
keeping the row listed and unseated if no word gets through, and as it starts
and each time it finds a core it settles what was left unsettled
(`Workspace::settle_openings`): a row made, the core is told again; none, it
is told the row was not made; a row whose helper the core no longer holds for
an opening is let go of. One opening of a helper at a time: a click's and the
settling's share `Workspace::begin_opening`. `promoting` and since when are saved in
`helpers.json`, so a core restarted meanwhile holds the helper the same way. An
opening unsettled for 15 minutes (`PROMOTE_TIMEOUT_MS`, checked at each read of
the records, `lives`) goes back to the parent, which hears so (`handover::opening_lapsed`);
a row made for it after that is refused (`background_promoted` answers false)
and the window lets go of it.

## Worktrees

Luna makes, inspects and removes no worktree for a helper
([ADR-0030](../adr/0030-scope-of-0-20.md)): `luna_session_spawn` takes no
`worktree`, and a caller that wants isolation makes one with `git worktree add`
and passes it as `folder`. A helper handed a worktree runs in it, filed under
its project (`SpawnTaken::in_worktree`). `Entry::cwd` is where its CLI says it
works (its first `init`'s cwd; a later one says where the shell went, `home_of`
maps it back): a turn with no conversation yet starts there, and a conversation
resumes from the folder it ran in, as Claude Code finds it by that folder. A
`helpers.json` of 0.19 that has a worktree's name or flag in a spec reads all
the same: they are let go.

## Outliving the core

### Background helpers: `helpers.json`

Every 2 s, as the core leaves (`stop_all`), and at once on what a killed core
must not undo (a spawn, a conversation named, a delete, a question, a
handover), the registry is written to `<data>/helpers.json` when changed, one
save at a time. It holds the spec without its token, the session id, the
turn count, queued text with each sender (`Queued`; a `send` or
`interrupt` past 16 MB waiting for the helper, queued or joined on its stdin
and not yet taken in, is refused, saying why, `room_for`), and the
agent ties (`agents::Ties`: parent, name, settings, debts, counts shown, all
helpers' read in one hold of the agent registry, `ties_of_each`); the
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
  `undelivered`. `luna_session_stop` drops the queue. Text that waited
  through a hold (`go_on`: an opening as a chat or a delete)
  or through a stop ending what its turns left (`kill`), and whose turn
  then cannot start, is kept the same way (`run_waited`).
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
- A context line (usage.rs `budget_news`) is marked in the notice itself (`Queued::budget`), so
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
Each save takes its snapshot and writes it under one lock (`save_in_order`),
so the newest snapshot is the one left on disk, and the freeze takes the same
lock: a save under way is written before the stopping starts, and none after.
Each stop holds the freeze for its own reason (`Stop`) until its caller knows
the process goes on, and only then lifts it (`thaw_chats`), once no other
stop holds it: an update whose install failed (the window's
`update_called_off`, which, said while the sessions are still being stopped,
takes effect as the stop returns), a PC shutdown the OS is seen to take back
(power `start_over`). An update's freeze is lifted too by a panic in its
stop, and by a window of the core's own build coming once the window that
asked for the stop is gone, which says no install came; never by the clock,
as an installer may wait on UAC for as long as nobody answers. It is kept on
disk while it stands, so a core of the same build started meanwhile starts
with it ([power and lifecycle](power-and-lifecycle.md#updates)). A call-off
said before the update's stop has begun (the window reconnecting during the
save `stop_all_sessions` makes first) is not remembered. The core leaving never
lifts it, and neither does a shutdown
whose fate is unclear, whose chats stay closed for the next core to take up.
A session starting or running meanwhile (a pane asking for its chat again, a
helper's turn) is no sign the process goes on, and lifts nothing: a save
then would write what the stopping did.

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
never completed, prompt in a file (`never_completed`).

## Invariants

- No turn starts after `stop_all`; `stop_all_sessions` before an update stops
  turns too and ends what helpers' turns left running (`stop_for_update`), and
  a core says which of its sessions are only such leftovers
  (`running_counts`, `Welcome.left_only`), or the updated window, counting
  them as sessions, would not replace it. No turn starts from before that stop
  lists the turns until the install, or the update is called off (the start
  gate, `starts.rs`, [power and lifecycle](power-and-lifecycle.md#updates)): a
  turn's claim is in the registry before it asks the gate, so the stop either
  stops it or it is refused, its batch queued again (`requeue_refused`; a
  send to an idle helper is its turn's batch, `in_flight`, like queued text)
  and a new helper's spawn refused. Nor does text go onto a running turn's or
  a listening lead's stdin meanwhile: it is queued. The stop waits only for what it stopped
  (`Run::claim`), ends only what helpers between turns left, and leaves a
  turn stopped and not over as the session it is. Off Windows a job lists nothing, and there is nothing to end.
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
place), and chat text in `luna.log`, which names sessions. A helper's turn that
failed (its process gone with anything but success, or its stream saying it
failed) is logged with its cause, on one line (run.rs `log_failed_end`), with
what may be a secret taken out (`redacted`: the word after `Bearer`, a URL's
path and query, any run of 24 or more letters, digits, `-` and `_` with a
digit in it): how
the process went, the error its stream gave (Claude Code's `result` text or
subtype, Codex's `turn.failed`, or Luna's own stop's reason), at most 200
characters, and the last 1,000 bytes of its stderr; the CLI's own words, so a
caller's "failed" has a cause on record. Luna's own stop is logged as info,
any other as a warning.

## Failure modes

| Failure | Behavior |
| --- | --- |
| Core killed mid-turn | turn counts as ended; caller told "did not finish"; optional auto-resume |
| Core killed between claim and stdin | `in_flight` neither forgotten nor replayed |
| Disk full | letting go of a tied job is one unlink (`tied-jobs/`), which a full disk does not refuse |
| Claude Code does not stop a turn it was asked to | killed 5 s after the ask with no `result`, or 5 s after the `result` with the process still up, or at once once the helper is deleted under the wait, which says nothing of its process (`end_turn_process`, `kills_after_asking`) |
| Claude Code stops a turn as asked and goes, a tool's child left running | what that CLI started in the helper's job is ended at the turn's end, as the kill would have ended it (`turn_over`, `ends_its_tree`, `procs::end_tree_of`): on Windows by parent links, elsewhere by the turn's process group and session (`keeper::listed_of`). Residual: on Windows a worker whose launcher went before it is spared, and one outside the job is not seen; elsewhere one that left the group and session stays, in the job by its mark; a turn in no job has nothing to walk it by |
| An ask's stdin closed under it | the CLI fails the ask itself; the held ask goes with the turn, or with `LISTEN_MS`'s close |
| Window gone while opening one as a chat | the next window settles it by whether the row exists |