# Power and lifecycle

How Luna knows what each session is doing, keeps the PC awake for it, powers
off when everything is done, and how its own processes start, update and end.

## Responsibility

- Keep one lifecycle record per session, chats and helpers of both CLIs, made
  of what the CLIs themselves say, and derive every view of liveness from it.
- Hold the PC awake exactly as long as a session is active, and fire the
  "when done" rule only when none has been for the whole quiet window.
- Make sure nothing a session started runs on unowned after it is gone.
- Start, link, update and end the window and core processes, and say why a run
  ended.

## Key modules

| Path | What it holds |
| --- | --- |
| `activity/record.rs` | the record (`Life`, `Phase`, `Outcome`, `Background`) and how each CLI's words fold into it |
| `activity/mod.rs` | each terminal session's track and record, the turn count (`turns_ended`, `keep_count`), the views (`TurnState`, `SessionLife`, `Summary`) |
| `activity/hooks.rs` | the hooks of either CLI's chats, applied to the record at once |
| `codex/hooks.rs` | Luna's status hooks for a Codex chat, as `-c` overrides, each trusted by a `hooks.state` entry of its hash |
| `activity/reconcile.rs` | the 5 s read of the disk and the jobs, the board power/ follows |
| `background/rows.rs` | a helper's record (`life_of`), every helper's (`lives`) |
| `procs/` | Job Object per session, its members and their start times, leftovers; `win.rs`, `unix.rs` |
| `keeper/` | off Windows: the `--keep-ties` process, job records, `end_last_ties`, `end_tree` |
| `power/` | `mod.rs` state; `rule.rs` each read; `machine.rs` firing and OS calls; `admission.rs` what a draining rule lets in; `commands.rs` incl. `luna_power_set` |
| `coreproc.rs`, `corelink.rs`, `ipc.rs` | the core's life, the window's link, the frame protocol |
| `elevation.rs` | no CLI as administrator: the window's de-elevated core start, the refusal everywhere a CLI starts |
| `lifecycle.rs` | exit reasons, the `--watch-exit` watcher, `luna.run`, Restart window |
| `log.rs` | `luna.log` |
| `watchdog.rs` | rebuilds a window whose WebView died |

## The lifecycle record

One `activity::Life` per session ([ADR-0031](../adr/0031-one-lifecycle-record-per-session.md)):

| Field | What it says |
| --- | --- |
| `phase` | `running` (a turn runs), `asking` (a turn, or a subagent of it, is held on a question or permission), `ended` (no turn runs), `unknown` (nothing read yet, or what was read does not say) |
| `outcome`, `error` | between turns, how the last one ended: `ok`, `failed`, `usageLimit`, with the CLI's error word, or `stopped` with the reason Luna stopped it (a helper's: `luna_session_stop`, Stop, a send with `interrupt`) |
| `turnStartedMs` | while a turn runs or asks, when it began |
| `background` | what runs beside the turns: `shells`, `crons`, `subagents`, and `names` where Luna can tell ("tail.exe (pid 22104)") |
| `turnsEnded` | turns ended, across respawns and resumes; only grows (`keep_count`, `count_at_least`) |

`Life::active` is the one liveness test: any phase but `ended`, or anything in
`background`. **Unknown is active.** Nothing is measured: no CPU, no I/O, no
output, no screen. What a CLI does not say is not seen.

### Where each CLI's record comes from

- **Claude Code, a chat.** Its hooks, posted to the hub through the
  `--settings` file and applied at once (`on_hook`): `UserPromptSubmit`,
  `PostToolUse` → running; `PermissionRequest`, `PreToolUse` of
  `AskUserQuestion`, a `Notification` of `permission_prompt` or
  `elicitation_dialog` → asking; `Stop` → ended, with what it lists of
  `background_tasks` (shells, background agents) and `session_crons` kept as
  the record's shells and crons until the next `Stop`, `SessionEnd` or a
  respawn; `StopFailure` → ended, failed or usage limit. A subagent's hooks
  carry its `agent_id` and are never the turn's: from `SubagentStart` (or its
  first tool) to `SubagentStop` it counts in `subagents`; one on a prompt of
  its own makes the record `asking`; each is the session's latest word, as a
  turn's hook is (`hook_at_ms`). Every 5 s the registry
  `sessions/<pid>.json` is read (`claude::session::status`): its status word
  (`turn_of`), when it was set (`statusUpdatedAt`), and the background shells it
  names (`shell`, 2.1.327 on; a list, an object or a count). It settles a hook
  missed: the registry beats an "idle" from a hook; a hook's "waiting" beats
  the registry's "busy" (a permission prompt is inside a turn); the registry's
  `waiting` beats a hook's "busy". No mark is eternal, and each ages only
  from the later of the two words: a hook's "busy" against a registry idle for
  15 s since the hook is a `Stop` missed, and that turn counts as ended; a
  hook's "waiting" at once when the registry's idle is newer; but an idle
  overrules a hook only when the registry set it after that hook
  (`statusUpdatedAt`): an idle from before, or of no known time, never ends
  the turn a hook began, however long it is read. The registry's "busy"
  against a hook's "idle" stands when the registry set it after the hook (a
  turn whose start hook was missed), else is a registry left behind after
  60 s. An idle registry set after the last hook, a subagent's included, and
  read for 15 s means no subagent is left. A `Stop` that arrives after a read
  counted its turn as ended (`read_counted`) counts nothing. The registry entry is the one named
  by the CLI's pid, else the one resumed, else the one live `cli` entry for
  its folder and start; with several live candidates none is taken and,
  without hooks, the record is `unknown` (one chat must not read another's
  status). A registry that says a dialog is up while the
  transcript's last line is an API error (`last_api_error`) is a turn ended
  on that error, not a question. A turn whose end no hook said (a `Stop`
  missed, a start hook missed, no hooks at all) counts as its record goes from
  a turn to none; a registry left busy after a `Stop` begins and ends none.
  Its shells are what its job holds that its turns started (below), and what
  its last `Stop` and its registry name (2.1.327): a `Stop` count goes once a
  look finds nothing in the job and a registry read that names none; a
  registry that does not read leaves it (a background agent is in no job).
- **Codex, a chat.** Luna's status hooks and its rollout. The hooks
  (`codex/hooks.rs`), given at launch as `-c hooks.<Event>` and trusted one by
  one for that run by a `-c hooks.state` entry naming each by its key and the
  hash Codex computes for it (nothing written into the account, whose own
  `hooks.state` stays its own), post to the hub as a Claude Code chat's do,
  the URL in `LUNA_HOOK_URL`, by the curl at an absolute path resolved at the
  launch (`codex_hook_curl`), with no curlrc and no proxy. Its rollout, read
  every 5 s by its own boundaries (`codex::session::rollout_read`):
  `task_started` with no end yet → running, or asking while a
  `request_user_input` holds the turn; `task_complete`, `turn_aborted` → ended,
  failed when the rollout says why, usage limit when `rollout_limit` names
  one; its count of completed turns is the turn count. The rollout alone ends
  a turn (`fold_codex`): Codex runs `Stop` before it decides to go on or to
  compact, so a `Stop` or an `Interrupt` only moves the activity stamp. The
  hooks give what the rollout cannot. A prompt (`UserPromptSubmit`) later
  than the latest turn end any read showed (`prompt_ms`, `end_seen_ms`) is a
  turn running until the rollout writes an end after it, so an older end a
  rollout whose writes stopped still shows never masks it; a tool's hook says
  running while the rollout says nothing (none yet, unreadable, no turn
  read); else a `Stop` there leaves it not known. A `PermissionRequest`
  (`asked`, by the command it names; 0.169.0 gives no call id) is asking
  whatever the rollout says of the turn, until the call of exactly that
  command has its output in the rollout (a long command approved: the output
  its yield writes; a denied one: its refusal) or its `PostToolUse` comes, a
  turn ends after it, `Stop` or `Interrupt` (the turn's model done), or the
  next prompt; one that named no command is answered by any tool's output
  after it; a parallel call's output, or a command only like it, answers
  nothing. Under `--approve-for-me` it is asking while the reviewer model
  decides. An approval dialog on its screen (`codex::session::approval_on`,
  looked for while the rollout says a turn runs) is asking too, whatever hooks
  came: the `PermissionRequest` post may be the one lost. Hooks that do not
  run at all (the listener down, `features.hooks` off in the account, no curl
  or one older than 8.3) leave the rollout and the screen, as before them.
  Which lines are this process's is told by position, never by the clock:
  for the thread it resumed, the file's length at the spawn
  (`codex::session::note_spawn`); a line past it is new whatever its date.
  Elsewhere a line's date against the spawn decides, and a `task_started`
  dated before it is in doubt (`Scan::doubt`), `unknown`: an earlier
  process's turn, or this one's under a wall clock set back.
  Missing evidence is `unknown`, never ended: no rollout found (it appears
  with the first turn, so a chat just started has none, and holds the PC
  awake until its first turn ends: no hook of Codex's speaks before its first
  prompt, `SessionStart` firing inside that turn), one
  that will not open or breaks off mid-read (`pty::lines_from`; the scan read
  before is no word on what came since), or one with no turn event read yet.
  Its subagents come off their own rollouts (`children_at_work`): one begun
  dated before the spawn counts when it was not there at the first look; one
  found is kept for the process's life, past the two weeks of folders looked
  at; a folder that will not list or a subagent's rollout that will not read
  is one subagent not seen (`NOT_SEEN`), at work; one a hook said is on a
  prompt of its own stays so across reads until its next hook. Its shells: the
  ones its rollout says its turns left running (`codex::session::Shells`: code
  mode's printed `exec_command` result with a `session_id` and no `exit_code`,
  or the plain tool's `Process running with session ID N`, until the
  `CommandExecution` item of that `process_id` completes), of this process's
  lines only, and only off the output of a call that runs a command (an
  `exec_command` call, or a cell whose script calls it), matched by its call
  id; named by their command, counted beside what its job holds. One stands
  until its end is written, whatever the job shows (a remote executor's
  process is in no job of Luna's), or until the CLI's exit: its `SessionEnd`
  turns the record `unknown` (active) until the first read taken after it
  (`exited_ms`, `settle_exit`), which takes the claims it shows as the exited
  process's and lets the rollout's own word on the turn stand. A claim is never
  ended by the clock. So a Codex 0.169 chat whose legacy rollout writes no
  `CommandExecution` end stays awake, its claim standing, until its Codex
  process exits. A result printed by a call
  made before an end starts nothing; one made after it is a new process, the
  id reused.
- **Both CLIs' chats: the job.** Every read lists what the session's job
  holds besides its CLI (`procs::in_job`: the CLIs' own helpers
  `codex-code-mode-host` and `conhost` left out; a program with a window of
  its own counts, as `Start-Process python server.py` and `start cmd /k npm run
  dev` show one and are a turn's work all the same). Until the first prompt of a CLI process goes in, all of it
  is the CLI's own furniture, its MCP servers (`Track::furniture`, snapshot at
  every read until then, and last at the first of: a person's Enter on a
  draft, Luna's text sent, Claude Code's `UserPromptSubmit`
  (`activity::submitting`); never at a later hook, a `PostToolUse` whose
  prompt's hook was missed, nor at a read once a turn is seen); where start times are epoch ms
  (Windows) what was born before the first turn's own start (the hook's time,
  the rollout's `task_started`) is furniture too. The rest are shells, by name,
  for as long as they are there, whatever the CLI version says. A job that will
  not list is `background.unlisted`: not known, active. Off Windows the job is
  the keeper's (`keeper::listed`): the CLI and what hangs from it by parent,
  group or session, and what carries the session's mark (`LUNA_JOB`, set on a
  chat's CLI at its spawn as on a helper's turns: `keeper::marked`).
- **A helper, either CLI.** Its turn's process and stream-json events
  (`background::life_of`): a turn's process → running, a question or
  permission its CLI asked on its stdout held → asking, its process gone → ended,
  failed with the error it ended on (a limit as its word, outcome
  `usageLimit`: a Codex helper's off the message its `exec` stream ends on,
  `codex::session::limit_of_message`; a Claude Code helper's off its stream's
  stand-in `assistant` message's `error`, `rate_limit` as a chat's
  `StopFailure` says it). Between turns, every process its turns
  left in its job (`procs::left_in`) is a shell, active for as long as it is
  there; there is no hold to run out. Re-read at every read of the records
  (`lives`) and at a turn's end; a job that will not list is
  `background.unlisted`, and so is a turn whose process could not be put in
  the job (`procs::adopt` or, on Windows, `join` refused: `Entry::outside`),
  from its end until a walk below its CLI by parent links, of processes born
  since the turn began, finds nothing left (`procs::left_below`; off Windows
  there is no such walk, and it stays not known). Taken up after a core
  restart, what its turns left under the last core is in no job this core
  has: seen out with that core's tie, or by `stop_all` as it left (saved as
  `left_seen_out`), it is known gone, and stays so through every core after
  until its next turn (`seen_when_taken_up`, `left_seen_out`); else it is not
  known, and active, until its next turn's job says what it holds.

### Views

Every consumer reads the record, and none keeps a verdict of its own:

| Consumer | Its view |
| --- | --- |
| The power rule and keep-awake | `Summary.busy`: sessions active; `last_activity`: the one monotonic activity stamp (below) |
| `luna_session_wait`, `read`, the notices ("NOT done yet", "Nothing of its own is running") | `TurnState::of`: `turn` (`Life::turn`; unknown reads busy), `busy` (turn not over), `turns_ended`, `children_busy` (`Background::any`) |
| The rows' status (`sessions_meta`) | `Life::status`: `busy`, `waiting`, `background` (turn over, work beside it, or an answer owed it), `idle`; the disk's own word until a record exists |
| `luna_session_get`'s `leftRunning` | the background's names, else its counts |
| The person's Delete dialog (`at_stake`) | a helper's phase and background |
| autoDelete (`reap`) | not active, for 30 min |
| The core's idle exit, `Welcome.sessions` and `left_only`, `close_if_left_only` | helpers active (`background::running_counts`); left only = active between turns, with only processes left in its job |

## Process jobs

Only a helper's job is tied to the core (`procs::tie`,
`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`): a chat's CLI goes with its terminal when
a core dies, a helper's has none and would run on unowned. Every orderly exit
lets go first (`let_go`, `Drop for Job`); only a core's death kills windows a
helper's turn opened. When a session goes (delete, kill, Quit, a respawn),
`procs::end_leftovers` ends what is still in its job, sparing windowed programs
(`listed_to_end`: the one place a window is looked for, since a program with one
is the user's to keep; liveness counts it, so what is spared still holds the
session active until it is closed).

Off Windows the tie is the **keeper** (`keeper/`): this exe with `--keep-ties`,
one per core once a helper has a turn, reading a pipe only the core writes; EOF
is the core's death, on which it ends every job not let go of. It also checks the
core by pid and start time every 2 s (on macOS a pipe becomes close-on-exec a
moment late). Not `PR_SET_PDEATHSIG`: it fires when the spawning thread ends,
reaches one child, and macOS has none. A job there is what carries its mark
(`LUNA_JOB`; for Codex also `CODEX_THREAD_ID`, set after its environment
filter), the turns' processes by pid and start time with their groups, and all
below by parent link. Stops go top down; on Linux each signal through a pidfd,
never to a group by number. Each tied job is a file in `<data>/tied-jobs/`, so letting go is one unlink a
full disk does not refuse; the next core ends what is left before it takes
helpers up (`end_last_ties`). A pid whose start time differs is another's and
left alone, a file of another boot names nothing, and a job not seen out stays
written down for the core after. A job lists there through the keeper's own
walk (`keeper::listed`), and `end_leftovers` ends what it lists, each by pid
and start time (`keeper::end_listed`); nothing tells a window from a job
there, so none is spared. `kill_tree` there is `keeper::end_tree`, the equivalent of `taskkill /T`: the
process, what it started, and what is in a group or session one of those leads,
stopped top down and looked below again so nothing forks out, then killed. So
`luna_session_stop`, an interrupt, a core's stop and a chat that would not quit take the
tool commands with them. In the core a live pid that is no child of the core is
another program's and is left alone.

## Keep-awake and "when done"

- **The chip's state** (`power://state`) goes to the window only when it
  differs from the last one sent (`power::publish`). Its `summary.sessions`
  carry each record whole (`life`), with `busy` (active), `turn`, `procs`
  (what runs beside the turn) and `stalled` (the error an ended turn ended on).
- **Keep awake:** `PowerSetRequest(SystemRequired)` with a reason visible in
  `powercfg /requests` while any session is active, released 60 s after none
  is. An armed rule adds no hold of its own. Windows does not restart its idle
  timer for the request: past the sleep timeout it sleeps the moment the hold
  goes.
- **The activity stamp:** one moment on the monotonic clock
  (`activity::last_activity`, an `Instant`), moved by every signal: any hook,
  a read that found a session active or any input of a record changed (a
  registry word or its time, a rollout growing, a job's members, a turn
  count, a session gone), a helper's turn claimed or ended or its job
  changing (`touch`), an agent's call that may start work coming in or going
  (`power::enter`), a helper's turn about to start, Luna's text into a pane
  (`PtyManager::write`), a person's key into one (`write_session`, before it
  lands). Activity that comes and goes between two reads moves it
  all the same. The reader starts it over on a sleep or a clock step: the
  monotonic gap past six reads, or the wall clock off the monotonic one by
  30 s between two reads (`clock_moved`); the barrier checks the same over the
  countdown (`span_moved`: both clocks from its start, a suspend during a
  read being what the reader's check cannot see).
- **The rule:** no session active and the stamp a whole quiet window back
  (`quiet_s`, and from the arming at the earliest) → a 60 s countdown with a
  notification → due. Any activity after the countdown began, or a session or
  an agent's call (`admitted`) active at a read, cancels it, not the rule, and
  the window starts over from that activity. Due, it is decided behind a
  barrier (`power::barrier`): admission closes first, under the power lock
  (`State::closing`, and for whoever asks under other locks the start gate,
  `starts.rs`, which an update closes too: [Updates](#updates)):
  agents' sends, spawns and answers are refused with "Luna is shutting down",
  a helper's turn does not start (`spawn_turn`; its batch is queued again,
  not ended as an error, and started once admission opens: `requeue_refused`,
  `resume_waiting`), Luna's writes into panes are refused up front
  (`power::typing`: a notice or a send with its draft stash is one piece of
  work, let in before its first key and counted until its last, so text is
  never pasted and its Enter then refused). Each of those
  moves the stamp before it asks, so it either lands before the decision or
  finds the gate shut. Then a fresh read (`power::confirm`), then the stamp,
  read after it under the lock: it fires only with no session or call active
  and the stamp no later than the countdown's start, and neither clock having
  moved across it (a suspend or a clock step during a read: `Effect::Moved`);
  else admission opens again and the window starts over, also when the read
  or the firing panics (`Reopen`). Fired, admission stays closed until the firing
  is over (a refusal, a shutdown taken back, a wake). A person is never
  refused: their key moves the stamp, and the CLI's hooks do. The CLIs are
  closed and the OS asked only after the barrier. Every span is
  on the monotonic clock: a wall clock set forward shortens nothing. Unknown
  is active: better not to shut down than to shut down wrongly.
  Shutdown and restart ask the OS first (`power_off`, `machine.rs`): on Windows
  `InitiateShutdownW` with `SeShutdownPrivilege` turned on in the core's token
  first, `SHUTDOWN_POWEROFF` (or `SHUTDOWN_RESTART`) `| SHUTDOWN_FORCE_SELF`, a
  planned reason, and a grace of `SHUTDOWN_GRACE` (30 s). Not `SHUTDOWN_HYBRID`,
  so the PC shuts down fully whatever Fast Startup is set to; not
  `SHUTDOWN_RESTARTAPPS`; not `SHUTDOWN_FORCE_OTHERS`, so Windows refuses while
  another user is signed in. A refusal closes nothing and is logged with its
  Win32 error. The privilege is on for the call alone: its previous state is
  taken from `AdjustTokenPrivileges` and put back after. On a yes the core winds
  down inside the grace (`coreproc::wind_down`: helper turns stopped, the chats
  frozen until a cancellation is seen, each CLI asked to quit with
  `pty::GRACE`, 10 s). `InitiateShutdownW`
  returns at once and the shutdown follows on its own, or not at all, so what
  became of it is read off the OS (`Fate`), never off the clock: from the grace's
  end the core polls `GetSystemMetrics(SM_SHUTTINGDOWN)` ("nonzero if the
  current session is shutting down") and, seeing it, leaves the rest to Windows
  however long it takes; still zero `SHUTDOWN_OVERRUN` (60 s) past the grace, it
  reads the System log (`EvtQuery`) for the User32 shutdown events after the mark
  it took before asking (the latest `EventRecordID`, so no earlier cancellation
  and no clock change can pass for this request's), read to the record's end
  (only `ERROR_NO_MORE_ITEMS` is the end; an error or the cap short of it is no
  reading): its own 1074 — the record names the process by its exe and keeps the
  message passed, and both must be Luna's — then a 1075, which winlogon writes
  when an `AbortSystemShutdown` (`shutdown /a`) takes a pending shutdown back
  (`fate_of`). That order alone means canceled: the RunOnce
  value goes, the helpers' jobs are tied to the core again and the stopped ones
  settled — their stdin lines reconciled with the transcript off the lock, as
  `restore` does — and resumed (`background::reopen`, `settle_stopped`,
  `resume_stopped`), and the window starts every chat over (`pty://start-over`,
  `core.store.ts`), as a new core would. Another 1074 in between (another
  program's request, whose cancellation the 1075 may be), a 1075 only before the
  mark, or no reading at all undoes nothing: the RunOnce value stays, the chats
  stay closed, and a notification says how to bring them back by hand. Restart
  (Windows only, refused elsewhere at arming) sets the RunOnce value with the
  window's exe (`--window-exe`) before asking. Off Windows the ask is
  `shutdown -h|-r +1` run to its end with stderr in a private file of a fresh name
  in the data dir (`secret::create_private_rw`, never a planted link followed;
  not `.output()`: macOS's `shutdown` forks a child that keeps the pipe open for
  the whole delay), read back through its own handle, its exit status and what it
  said logged; nothing there tells a canceled shutdown from a slow one, so
  recovery is by hand. Sleep / hibernate is `SetSuspendState`; sessions live on,
  and the clock jump on wake resets the window and countdown.
- **Drain** (`Armed::drain`, "start nothing new") refuses agents' `luna_session_spawn`
  and `luna_session_send` in `mcp::run` and narrows `luna_session_answer` to structured answers;
  a person is never refused. Calls are counted under the lock that arms
  (`power::enter`); arming waits for those in flight (`settle`, bounded).
- The test build and unit tests never reach the OS (`power::dry`).
- An armed rule that does not fire logs `armed, not firing: <chat> <why>` on
  change (at most once a minute) and every 10 minutes; the hold logs
  `awake hold set: <who>` / `let go: … last held by <who>` and every hour what
  keeps it (`HOLD_LOG_EVERY_MS`).

## Core and window lifecycle

- The core: `--core`, one per data dir (`core.lock`), from a copy of the exe on
  Windows. It exits after 10 minutes with no window and no session; a helper in
  a turn counts, and so does one with what its turns left still in its job
  (`background::running`, `Life::active`). Leaving, it stops every helper's turn with its
  process tree (`background::stop_all`) and takes `core.json` (and off Windows
  its socket) along.
- **The endpoint** ([ADR-0021](../adr/0021-core-endpoint-by-os-identity.md)).
  The core listens on a name it draws afresh at each start (`endpoint/`): on
  Windows a named pipe, `\\.\pipe\luna-core-<32 hex>`, made with
  `FILE_FLAG_FIRST_PIPE_INSTANCE` and `PIPE_REJECT_REMOTE_CLIENTS`, a protected
  DACL of the user and SYSTEM (and Administrators, read and write) and a medium
  no-read-up, no-write-up label; one instance always waits for the next
  client, made before the last one is handed on, and each must read back the
  first one's DACL and label. Every handle is overlapped and not inheritable,
  so a read and a write wait at once on one connection, each to its own
  deadline. Off Windows, a socket `core-<16 hex>.sock` in `core/` (`0700`),
  stale ones swept at the core's start. `core.json` holds the core's pid, its
  creation time, the protocol (2) and the endpoint's name: no token.
- **Who is let in** (`peer/`), read before the first byte: on Windows the
  token the client opened the pipe with (the core impersonates the client
  for one `OpenThreadToken` and reverts at once; a window opens at
  identification level); on Linux `SO_PEERCRED` and `/proc/<pid>` (user
  namespace, `no_new_privs`, seccomp mode and filter count, a pidfd where the
  kernel gives one); on macOS the uid and pid. Refused, with nothing said back
  and the pid, image and reason logged: what cannot be read, a restricted or
  AppContainer token, another user namespace, `no_new_privs` where the core
  has none, a stricter seccomp state than the core's (a higher mode, or more
  filters: a heuristic that holds along the core's own chain, as for a sandbox
  it launches, not between launch contexts; ADR-0021), another user, a lower
  integrity. Then the stranger's limits: 32 connections before their hello, the
  hello within 5 s and 4 KiB. A core that itself runs with `no_new_privs`
  (started under Flatpak, systemd's NoNewPrivileges, a container) logs so once
  at its start: a client confined by Landlock alone in its namespace cannot be
  told from a window there.
- The window (`corelink.rs`) finds the core of its own build or starts it,
  holds the connection (losing it means the core died: banner, Restart), skips a
  `core.json` whose pid is dead, runs restarts one at a time. Before it says
  anything it checks that the endpoint's server is the pid `core.json` names. A
  core of another build with no sessions is replaced at once; with sessions only
  on a button. A core of an older version whose sessions were stopped for an
  update (`Welcome.stopped_for_update`: `Stop::Update` neither called off nor
  lapsed) and are all helpers between turns counted only for what their turns
  left running (`Welcome.left_only`, read with the count under one lock,
  `background::running_counts`) is replaced too (`Verdict::ReplaceLeftOnly`):
  the person agreed to that stop, and the core's going ends those jobs. One no
  update stopped (left running by a newer Luna installed by hand beside it, say)
  is asked about as ever. It is asked to stop only so (`ifLeftOnly`,
  coreproc.rs `left_only_now`): it looks again, closing helper turns off under
  the registry's lock as it looks, and refuses with a pane or a helper in a
  turn, which leaves it running for the window to ask about. A pane or a
  helper's turn is work to ask about; so is any session of a newer version's
  core or one of the same version built otherwise (a dev build), and of a core
  older than the field (0.19 and before), which says none are left only. While a core is
  being found, started or replaced (a restart, or another build's), the page's
  calls wait for it rather than fail, for a budget of the old core's stop and
  the new one's start (`LINK_WAIT`, 59 s, the sum in its comment; a budget, not
  a bound, as each step's timeout is per read or per call, `taskkill` given
  5 s, `pty::KILL_WAIT`); with no core coming (`down`, `mismatch`,
  `elevated`) they fail at once. A `core.json`
  with a token is a core of Luna 0.11 or before (protocol 1,
  `corelink/v1.rs`): the window says hello on its loopback port
  with that token, to learn its session count, and stops it if it has none;
  nothing else is ever sent it. `luna --stop-core` (the uninstaller's) goes
  through the endpoint like a window, or a protocol-1 core's port.
- **Never elevated** (`elevation.rs`). No CLI runs with administrator rights.
  A window that finds its own token elevated (or above medium integrity) starts
  the core with the desktop shell's token: `GetShellWindow`, its process's
  token checked to be this user's and not elevated, duplicated as a primary
  token and used with `CreateProcessWithTokenW` (the SeImpersonatePrivilege an
  elevated admin holds, and the Secondary Logon service), suspended until the
  new process's own token is seen to be ordinary. Its environment is the
  user's own from their profile (`CreateEnvironmentBlock` with that token, not
  the window's, whose PATH or TEMP an elevated start may point at an
  administrator's places), with the window's `LUNA_*` variables alone on top
  (a test build's dev caller); the working folder is the data dir. It is
  asked out of the window's job (`CREATE_BREAKAWAY_FROM_JOB`, retried
  without where a job forbids it); whether it landed in the window's job is
  checked while it is suspended and logged, not assumed: Secondary Logon
  creates it, and that path has not been run elevated here. No shell, a shell of another
  user (over-the-shoulder credentials) or an elevated one (UAC off, the
  built-in Administrator) and no core is started: link state `elevated`, the
  banner without a restart button. The core reads its own token too: elevated,
  it starts nothing of its own (no hub, no restored helpers), answers `info`
  and `shutdown` only, and says why in `Welcome.refused`; `pty::spawn`, a
  helper's turn and `codex debug models` (which the window runs too) refuse
  in an elevated process whatever asked; so do the unit and live tests, which
  read the OS's answer too (a test may add a refusal, never lift one; only a
  launch a test captures, never run, goes past). A running core whose token
  reads elevated or cannot be read at all (`ERROR_ACCESS_DENIED`: above this
  window's integrity; off Windows its effective uid, from `/proc/<pid>/status`
  or `ps`) is never attached (`verdict`): replaced with no session, else the
  banner asks for a restart. The peer check (`peer/`) turns a client of
  lower integrity away, which would leave an ordinary window no way to stop
  an elevated core; so a core that refuses work lets a plain one (not
  restricted, not an AppContainer) in to stop it and nothing else
  (`Verdict::StopOnly`): only `shutdown` is answered on that connection, and
  it is sent no events. An elevated administrator of another account is let in
  the same way, to stop it only. One that can still be neither reached nor
  stopped (an older build's, another user's) leaves, with no session, ten
  minutes after its last window; the banner says so. Off Windows, euid 0 is
  refused the same way, with no de-elevation.
- **Restart window** (tray) starts a new window as `--after <pid>`, the core left
  running. `watchdog.rs` rebuilds a window whose WebView died.
- **Quit** asks every session to quit as a delete would, stops helper turns, then
  the core and the window go; a core still there 30 s later is killed with its
  tree.
- **Why Luna stopped** (`lifecycle.rs`): its own exits with a reason (tray Quit,
  an update, OS sign-out or shutdown); the `--watch-exit <pid>` watcher logs the
  exit code (a kill from outside vs. a crash, and whether Task Manager was
  open); `luna.run` holds the last sign of life for the next launch, which
  reports event-loop hangs, machine sleep and reboots in between.

## Updates

- The app checks `latest.json` on GitHub Releases at startup (endpoint in
  `tauri.conf.json` → `plugins.updater.endpoints`); a release lights a chip and
  installs only on a click. A release whose `core_fingerprint` (a hash of
  what the core is compiled from: `src-tauri/src`, the manifest and lock, `build.rs`,
  and the system texts in `resources/` (the CLI floors are in `src/`); `fingerprint.rs`, run by `build.rs`) matches the running core restarts only the
  window. Any other stops the sessions (each asked to quit with its exit hooks,
  `stop_all_sessions` also stopping helper turns and then, once those turns are
  over or 15 s on, ending what every helper's turns left running,
  `background::stop_for_update`; a turn still not over then is left as it is,
  its job untouched, as its end may start what was queued behind it in that
  same job) and the chats resume from their transcripts. What a helper's turns
  left counts as a session (`background::running`, in `Welcome.sessions`), and
  as left only (`Welcome.left_only`), which the updated window replaces the old
  core over all the same (above), its going ending them: what a turn that
  ended late left among them. On Linux and macOS a job lists through the
  keeper's walk (`keeper::listed`), so what helpers' turns left is seen,
  counted and ended there too, every process of it, as nothing tells a window
  apart. Nothing new starts from before the stop lists anything until the
  installer ends the process: the start gate (`starts.rs`), which a power-off
  closes too, is closed with the freeze below (`freeze_for_update`) and opens
  as it lifts. A helper's turn asks it after its claim is in the registry
  (`spawn_turn`), so the stop either lists the claim and stops it or the turn
  is refused, its batch queued again (`requeue_refused`: the next core keeps
  it as undelivered, a call-off starts it, `resume_waiting`) and a new
  helper's spawn refused with "Luna is updating"; Luna's writes into panes
  are refused too (`pty::write`). A pane's launch, of either CLI, asks it
  before anything of the launch is built and is counted until it is
  registered (`starts::launching`): the stop waits for the launches under way,
  up to 10 s (`starts::seal`, `LAUNCH_WAIT`), before its snapshot, so each is
  stopped with the rest, and one that outlasts that is refused as it would
  register, its CLI asked to quit (`PtyManager::register`). A power-off does
  not refuse a pane: a person starts it. Text queued behind a turn the stop
  ended before it began is kept as undelivered (`run_turn`). A stop that
  panics calls the update off (`UpdateStopping`'s drop), so the gate opens at
  once. It is the outgoing core's code that stops the sessions and
  answers the hello: an update from 0.19 runs 0.19's `stop_all_sessions` and
  says no `left_only`, and may still ask for a restart once when a helper left
  processes running. The chats' ties stay frozen, and the gate closed, from
  that stop until the
  installer ends the process, until the window says the install failed
  (`update_called_off`; said while the stop is still under way, it takes
  effect as the stop returns), or until 10 minutes after the stop returned
  (`agents/persist.rs` `UPDATE_FREEZE_FOR`, on the monotonic clock), so an
  update that never came (the window gone mid-stop, the installer never
  started) leaves the core's saves off for at most that long. Nothing bounds
  how long a UAC prompt waits for an answer: one left past the bound lets the
  next save write what the stopping did, and the turns it cut are recorded as
  ended, a tradeoff taken for a bounded freeze. A call-off said before the
  stop has begun (the window reconnecting during the save that
  `stop_all_sessions` makes before it stops anything, `pty/control.rs`) is
  not remembered: that freeze lifts only by the bound.
- The CLIs update separately ([providers](providers.md#cli-binaries)).

## Logging

`luna.log` beside the exe (or in the data dir fallback) takes warnings, errors
and INFO events only: session start / exit / delete, helper turns, one line per
MCP call (caller, tool, target, outcome, duration, steering arguments, never
text), notices, the awake hold (INFO; the countdown and what fires it are
WARN). A helper turn's line gives the CLI's arguments with the chat's hook
URL, which carries its secret, as `<hook-url>`, and the
helper's identity line as its length (`background/run.rs` `args_for_log`).
Sessions are named by id and chat name (`agents::label`). Every line names the version; each launch opens with a
`START` line. The page's own lines (`append_log`) go to a writer thread of the
window's, in the order they came and stamped when they came, never written on
its main thread; past 1 MB waiting a line is refused, and the log says how many
were. The ways out wait up to 2 s for the lines handed over (`flush_page_lines`).

## Failure modes

| Failure | Behavior |
| --- | --- |
| A missed `Stop` hook | hook "busy" goes stale 15 s into an idle the registry set after the hook; its `Stop` arriving late counts nothing |
| A registry idle older than the hook, or of no known time | never ends the hook's turn nor drops a subagent whose hook came later: active until a later word |
| A forgotten `tail -F` or idle server | its session stays active, and holds the PC and a power-off, for as long as it runs: `luna_session_stop` between turns, or a delete, ends it |
| A hook missed and no registry to settle it, a rollout not found after a key was typed | `unknown`: active until the CLI says otherwise |
| A Codex turn on an approval dialog | reads `asking` from its `PermissionRequest` hook or from its screen: active either way |
| A Codex chat's hook not posted (curl failed, the listener busy) | nothing ends early: the rollout alone ends a turn; a lost `PermissionRequest` is read off the screen |
| A Codex rollout whose writes stopped after a turn, a new prompt in | running from the prompt's hook until an end after it is written |
| A Codex chat's rollout gone or unreadable after a `Stop` | `unknown`, active; never ended |
| A shell Codex's rollout names whose end it never wrote (0.169's legacy rollouts write no `CommandExecution` end) | counted, active, for as long as the Codex process lives, however long: nothing ends it by the clock. Its exit reads `unknown` until the next read settles it; the chat is the person's to stop |
| The wall clock set back after a Codex chat started | a turn begun past where the resumed rollout ended at the spawn reads running; elsewhere one dated before the spawn reads `unknown`; a subagent begun since, dated before it, counts as not at the first look |
| A rollout or a subagent's folder that will not open, list or read through | `unknown`, or a subagent not seen (at work); never the last scan's ended |
| A known subagent's rollout that is no longer there (not a read that failed) once the session's own turn has ended | that subagent is ended with the turn: Codex writes no end for a thread it archives. While the turn runs it stays not seen |
| A helper taken up with its old job not seen out, no job and no turn of its own to walk | `unknown`, active, until its next turn or a stop: `luna_session_stop` walks what it can (turns in no job, by parent) and, finding nothing, settles it as gone |
| Suspend during the barrier's read, or a clock step across the countdown | no firing; admission opens and the window starts over |
| A panic in the barrier's read or the firing | admission opens again, the firing over (`Reopen`) |
| A pane's launch under way as an update's stop begins | the stop waits for it, up to 10 s, and stops it with the rest; past that it is refused as it registers, its CLI asked to quit (`starts::seal`) |
| An update's stop panics, or the install fails | the update is called off and the start gate opens at once (`UpdateStopping`, `update_called_off`); an update that never comes opens it with the freeze's 10-minute bound |
| A helper turn that could not be put in its job | `background.unlisted` past its end, until a walk below its CLI finds nothing (off Windows a job is never refused) |
| A hook arrives, a key is pressed or Luna is asked to start work as a countdown runs out | the stamp moves or admission is shut first; the barrier cancels, or refuses the work |
| Core dies | chats' CLIs end with their terminals; helpers' jobs end with the core (Job Object or keeper) |
| Keeper gone with its core | the next core ends tied jobs from `tied-jobs/` (`end_last_ties`) |
| WebView2 dies | `watchdog.rs` rebuilds the window; sessions untouched |
| Luna started as administrator, and the shell's token cannot be had (no shell, another user's, elevated) or `CreateProcessWithTokenW` fails (Secondary Logon disabled) | no core started; the banner says why; nothing runs elevated |
| An elevated core runs that this window can neither reach nor stop | the banner says so; it has no session and leaves ten minutes after its last window, or an elevated `luna --stop-core` or a sign-out ends it |
| The OS refuses the shutdown or restart | nothing closed; the Win32 error logged and shown; a restart's RunOnce value removed |
| The shutdown is taken back after the sessions closed | the System log's User32 1074 then 1075 after the mark, read 60 s past the grace; RunOnce value removed, helpers' jobs tied again and the stopped ones resumed, every chat started over from its transcript |
| The shutdown has neither begun nor been recorded as canceled 60 s past the grace | nothing undone: RunOnce kept, chats closed; a notification says how to bring them back (restart the core, or Luna) |
| A shutdown taken back, then the core dies | its helpers' jobs were tied again on reopening, so their processes die with it; the next core's `tied` is true to the state |
| Restart's RunOnce cannot be set | logged; the PC restarts anyway |
