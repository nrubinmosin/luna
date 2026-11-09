# Power and lifecycle

How Luna decides a session is busy, keeps the PC awake for it, powers off when
everything is done, and how its own processes start, update and end.

## Responsibility

- Turn three noisy signals into one verdict per session: busy, waiting, idle,
  stalled.
- Hold the PC awake exactly as long as work moves, and fire the "when done" rule
  only when nothing is busy or waiting.
- Make sure nothing a session started runs on unowned after it is gone.
- Start, link, update and end the window and core processes, and say why a run
  ended.

## Key modules

| Path | What it holds |
| --- | --- |
| `activity/mod.rs` | per-session tracking, turn counts (`turns_ended`, `keep_count`), `Track::left` |
| `activity/hooks.rs` | Claude Code's hooks as turn signals |
| `activity/sample.rs` | the 5 s sampler: disk, processes, verdict |
| `procs/` | Job Object per session, CPU/IO per process, the busy rule (`judge`), leftovers; `win.rs`, `unix.rs` |
| `keeper/` | off Windows: the `--keep-ties` process, job records, `end_last_ties`, `end_tree` |
| `asking.rs` | a turn that ended on the waiting marker |
| `power/` | `mod.rs` state; `rule.rs` each tick; `machine.rs` firing and OS calls; `admission.rs` what a draining rule lets in; `commands.rs` incl. `luna_power_set` |
| `coreproc.rs`, `corelink.rs`, `ipc.rs` | the core's life, the window's link, the frame protocol |
| `elevation.rs` | no CLI as administrator: the window's de-elevated core start, the refusal everywhere a CLI starts |
| `lifecycle.rs` | exit reasons, the `--watch-exit` watcher, `luna.run`, Restart window |
| `log.rs` | `luna.log` |
| `watchdog.rs` | rebuilds a window whose WebView died |

## Busy is measured

Every 5 s `activity/` combines three signals per session:

1. **The turn.** Claude Code: hooks (via the `--settings` file, posting to the
   hub) checked against the registry. Codex: the rollout plus screen silence,
   and an approval dialog recognized on the rendered screen.
2. **Descendants** (`procs/`): at spawn the CLI goes into a Job Object that its
   descendants inherit. Busy = CPU/IO growth over the window, or a process born
   after the first turn began and still alive (how a background `sleep` is
   caught). What the CLI starts in its first 30 s (`STARTUP_MS`: MCP servers,
   its shell) is furniture; a program with a visible window of its own, and
   everything under it, is the user's and left out.
3. **Output** over the last 30 s.

Rules between hooks and registry: the registry beats an "idle" from a hook; a
hook's "waiting" beats the registry's "busy" (a permission prompt is inside a
turn); the registry's `waiting` beats a hook's "busy". No mark is eternal: a
hook's "busy" against an idle registry goes stale in 15 s; a hook's "waiting" at
once when the registry's idle is newer (`statusUpdatedAt`), else in 60 s; the
registry's "busy" against a hook's "idle" in 60 s without a hook. Without hooks
(Codex, a session that survived a restart) the registry is believed as is.

A session waiting on a dialog after an API error (`StopFailure`, or a last
transcript line with `isApiErrorMessage`) is **stalled**: not `waiting`.

**Subagents.** A subagent's hooks carry its `agent_id` and are never the turn's.
Luna keeps the ones at work apart, from `SubagentStart` (or a first tool) to
`SubagentStop`: while any is, the turn reads over (notices go in) but the
session holds the machine like a job (`children_busy`, "N subagents" in the
power chip). An idle registry for 15 s means none is left. Codex subagents are
read off their rollouts (`codex::session::children_at_work`, set replaced each
sample by `agents_on_disk`); the 15 s rule is Claude Code's only.

**Waiting by the marker** (`asking.rs`): a user's chat (not a helper) whose turn
is over and whose last message ends on a line holding the configured marker
reads `waiting` with `marked`. Cleared by hand (`dismiss_waiting`), it stays
cleared while the same words and time are shown. Activity keeps the turn idle,
so power counts it as done.

## Leftovers

Doing something is measured per process (`procs::leftovers`): CPU at 1% of a
core or 1 KiB/s of I/O between two samples at least 4 s apart (Windows counts
CPU in 15.6 ms ticks; a polling `tail -F` used 0.5 ms/s and 250 B/s). Birth is
not work: until 30 s old only CPU since start counts. What a turn left holds the
machine until it has done nothing for 2 h on end (`LEFTOVER_HOLD_MS`): a
Docker or remote client waits on work done elsewhere, but a Monitor's `tail -F`
once kept a PC awake all night. `luna_session_list`'s `active` is work in the last 10
min. When a session goes (delete, kill, Quit, a respawn), `procs::end_leftovers`
ends what is still in its job, sparing windowed programs.

## Process jobs

Only a helper's job is tied to the core (`procs::tie`,
`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`): a chat's CLI goes with its terminal when
a core dies, a helper's has none and would run on unowned. Every orderly exit
lets go first (`let_go`, `Drop for Job`); only a core's death kills windows a
helper's turn opened.

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
written down for the core after. Off Windows a job lists no
leftovers (nothing tells a window from a job), so `children_busy` stays false
there. `kill_tree` there is `keeper::end_tree`, the equivalent of `taskkill /T`: the
process, what it started, and what is in a group or session one of those leads,
stopped top down and looked below again so nothing forks out, then killed. So
`luna_session_stop`, an interrupt, a core's stop and a chat that would not quit take the
tool commands with them. In the core a live pid that is no child of the core is
another program's and is left alone.

## Keep-awake and "when done"

- **Keep awake:** `PowerSetRequest(SystemRequired)` with a reason visible in
  `powercfg /requests` while busy > 0, released 60 s after quiet. `waiting` does
  not hold the machine but blocks shutdown; stalled does neither. While the rule
  is armed the hold is always kept. Windows does not restart its idle timer for
  the request: past the sleep timeout it sleeps the moment the hold goes.
- **The automaton:** busy = 0 and waiting = 0 for `quiet_s` → a 60 s countdown
  with a notification → action. Any activity cancels the count, not the rule.
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
  frozen, each CLI asked to quit with `pty::GRACE`, 10 s). `InitiateShutdownW`
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

- The core: `--core`, one per data dir (`core.lock`), 127.0.0.1 with a token in
  `<data>/core/core.json`, from a copy of the exe on Windows. It exits after 10
  minutes with no window and no session; a helper in a turn counts, and so does
  one whose leftovers still hold the PC (`background::running`). Leaving, it
  stops every helper's turn with its process tree (`background::stop_all`).
- The window (`corelink.rs`) finds the core of its own build or starts it,
  holds the connection (losing it means the core died: banner, Restart), skips a
  `core.json` whose pid is dead, runs restarts one at a time. A core of another
  build with no sessions is replaced at once; with sessions only on a button.
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
  banner asks for a restart. The peer check (`peer.rs`) turns a client of
  lower integrity away, which would leave an ordinary window no way to stop
  an elevated core; so a core that refuses work lets a plain one (not
  restricted, not an AppContainer) in to stop it and nothing else
  (`Verdict::StopOnly`): only `shutdown` is answered on that connection, and
  it is sent no events. One that can still be neither reached nor stopped (an
  older build's, another user's) leaves, with no session, ten minutes after
  its last window; the banner says so. Off Windows, euid 0 is refused the
  same way, with no de-elevation.
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
  what the core is compiled from: `src-tauri/src`, the manifest and lock, `build.rs`
  and `cli-known-good.json`; `fingerprint.rs`, run by `build.rs`) matches the running core restarts only the
  window. Any other stops the sessions (each asked to quit with its exit hooks,
  `stop_all_sessions` also stopping helper turns) and the chats resume from
  their transcripts.
- The CLIs update separately ([providers](providers.md#cli-binaries)).

## Logging

`luna.log` beside the exe (or in the data dir fallback) takes warnings, errors
and INFO events only: session start / exit / delete, helper turns, one line per
MCP call (caller, tool, target, outcome, duration, steering arguments, never
text), notices, the awake hold. Sessions are named by id and chat name
(`agents::label`). Every line names the version; each launch opens with a
`START` line.

## Failure modes

| Failure | Behavior |
| --- | --- |
| A missed `Stop` hook | hook "busy" goes stale in 15 s against an idle registry |
| A forgotten `tail -F` or idle server | holds the PC 2 h after its last activity, then logged and let go |
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
