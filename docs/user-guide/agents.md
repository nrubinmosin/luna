# Agents: sessions that run sessions

A chat created with **Luna tools** ticked (off by default) gets Luna as an MCP
server, and with it sixteen tools, each named for the thing it acts on and what it does
(`luna_<resource>_<verb>`; a `list` gives compact rows, a `get` one item whole):
`luna_session_list`, `luna_session_get`, `luna_session_spawn`, `luna_session_send`,
`luna_session_answer`, `luna_session_wait`, `luna_session_stop`, `luna_session_delete`,
`luna_message_list`, `luna_report_get`, `luna_account_list`, `luna_account_get`,
`luna_run_list`, `luna_guide_get`, `luna_notification_send`, `luna_power_set`. The names
before Luna 0.11 (`luna_list`, `luna_spawn` and the rest) are gone: called, one answers with
the tools there are now.

The model can start a helper session (another model, another account, Codex
instead of Claude) with an opening prompt, wait for its reply, send follow-ups,
answer what it asks, and delete it when done. Luna does not create or remove
worktrees: for isolation the agent creates one itself with `git worktree add`,
passes it as the helper's `folder`, and removes it when done. The tools and a short instruction cost
about 2,500 tokens with every request of a tooled session ([what Luna adds to a
model's context](context.md)); the manual is `luna_guide_get`'s, read only when
the model asks, a topic at a time (a tool's name, `notices`, `restart`,
`helpers`), or with no topic its opening and the topics. A chat without the box
knows nothing of Luna, and neither does a background helper spawned without
tools. Such a chat can still receive: another of your chats with Luna tools
can type into it and answer its model's questions with `scope: "all"`
([Talking to sessions](#talking-to-sessions)).

- Arguments past the obvious: `luna_session_wait` waits `until` a turn is done
  (`turn_done`, the default), the session waits (`waiting`) or it exits
  (`exit`), for `timeoutSec` (600 unless set, 1,800 at most), and `afterTurn`
  skips turns already seen. `luna_message_list` gives the newest `last`
  messages (1 unless set) or those after `since`, as many as fit in one
  answer (the rest from the cursor it returns), `toolCalls` marking a Claude
  Code session's tool calls. `luna_session_list` filters by `ids`, `state`,
  `provider`, `account`, `group` and `q`, 50 rows at a time (`limit`, `cursor`);
  `luna_run_list` searches with `q`, 10 runs unless `limit` says otherwise. A
  spawn, a send and a delete take a `requestKey`, so a retry with the same key
  and arguments does not do it twice while the core keeps the first result
  (up to an hour, never past a core restart). `luna_session_answer` refusing a permission (`allow: false`) takes a
  `note` saying why.
- `luna_notification_send` raises a desktop notification mid-turn, titled with the chat's
  name and opening it when clicked, at most one every 10 seconds.
- `luna_power_set`, for when you ask for it, arms the same "when done" shutdown,
  restart, hibernate or sleep as the power chip, which shows and cancels it; the
  action `cancel` disarms it too.
- Past a share of its context window (**Tell me when a helper's context
  passes** in Settings → Agents & trust → Helpers, 50% unless changed; a spawn's
  `budgetPct` sets one helper's), every notice about a helper ends "(context
  N%)"; past 85% its parent hears so once on its own. Nothing is stopped.
  `budgetTokens` still works, read as the share of the window it is.
- A lead (a helper given Luna's tools) does not wake its parent with what it
  writes while its own helpers or background work are still at it; that waits
  for the notice that comes once they are done, unless a message opens on
  `DONE`, `BLOCKED`, `QUESTION`, `FAILED` or `PLAN`. Once they are all done and
  it has said nothing, Luna wakes the lead itself, once, and its parent hears
  what came of that, or that it seems stuck.
- When a helper or lead ends a turn with nothing of its own running (no job,
  subagent or helper of its own still at it,
  nothing waiting to go into it, a usage or context notice included),
  its agent's notice adds "Nothing of its own is running: it will not go on
  unless you send to it", whatever its last words said it was waiting for.
- A send may ask for no reply (`expectReply: false`): the helper's answer then
  wakes the agent only for a status word or a report.
- An agent reads, waits on, sends text to and answers its own helpers. One of
  your own chats reads and waits on your other chats and other chats' helpers
  only by saying so (`scope: "all"`). It sends text to and answers only your
  other chats that way, and what it sends them, an answer typed in as a
  message too, arrives opened `[From session <id> via Luna]`, so that chat
  knows the words are not yours. Another chat's helper takes text and answers
  only from the sessions above it (its chat, and a lead it works under): a
  chat that tries is refused and told whose the helper is. A helper with no
  chat takes text from no agent. A chat that writes to another is told when the answer to its text
  comes, as a parent is. A helper, even one given Luna's tools, reaches
  only the helpers under it: never your chats, its own parent or another
  chat's helpers, and it cannot send desktop notifications or set the PC's
  power. What it has for anyone else goes in its answer. Only its own helpers
  can an agent interrupt, stop or delete.
- A helper starts without its account's MCP servers and connectors (claude.ai's
  Gmail, Slack and the like; Codex's apps and the servers in its account's
  `config.toml`), and a Claude Code helper with auto-memory off, so that what
  an agent briefs it to do stays inside Luna. A spawn with `connectors: true`
  keeps them. A Codex helper also goes without the servers of the project's
  `.codex/` folders and of Codex's machine-wide config, and without plugins
  but the library's (their skills too, as a plugin goes whole). One whose
  config Luna cannot read, or whose managed config sets servers, plugins or
  apps (which no single run can turn off), is not started without
  `connectors: true`.
- A tool called with an argument it does not take, or of the wrong type, is
  refused with what it does take, rather than run on a default.

## Helpers

- Every helper runs in the **background**: no row, no pane, no terminal. Each
  turn is one run of the CLI's headless mode (`claude -p`, `codex app-server`); the
  next resumes the conversation. It needs no open window. Its parent's pane
  shows a tag in its title bar with how many work, how many are idle and how
  many ask: `2 working · 1 idle · !1 asks`, or `3 !1` in a narrow pane. A
  helper that asks counts as working: its turn waits on the answer. One that
  has exited is not counted.
- A chat that a helper, or a helper's helper, still owes an answer reads
  **background** between turns ([Chats: Status](chats.md#status)), also while
  that helper asks something: the tag counts it, and the list shows the
  question.
  Helpers another chat started never color it.
- A click on that tag opens the chat's helpers, **Background helpers of this
  chat (N)**, over the board (the status bar's **background** count opens every
  chat's, **Background helpers (N)**), in two tabs, **Working** and **Idle**:
  Working first when any works, asking helpers at its head and framed. Each
  helper shows what it is doing and for how long, its model, effort and
  account, when it was last active, the first line of its task and what it
  asks, and offers:
  - **Watch:** its conversation, read-only, as it goes: what its agent asked and
    what it said, tool calls marked, updating every second while a turn runs.
    A long conversation shows its latest part, and very long entries (a tool's
    whole output) are left out; the view says when it did either. At its end it
    follows what comes in; scrolled up to read, it stays where you are, also as
    its oldest messages make way for new ones. Nothing typed there reaches it.
  - **Open as chat** (between turns, once nothing it left runs, and not while
    a turn from before Luna last restarted may still be running): it becomes a
    chat of your own, for good, a row in its folder seated in a pane, going on
    with the same conversation. Its agent loses it: it can no longer send to it,
    wait on it, read it as its own, stop or delete it, and is owed nothing more.
    The agent hears so once, in a notice that survives a restart. Text still
    queued for the helper is not delivered: whoever sent it is told, and the
    notice names the file it is in. Luna asks before it does this. If Luna
    cannot finish opening it (the window or the core closed midway), it
    finishes when the window starts again or finds the core again: the chat
    opens if its row was made, and the helper stays its agent's if not. If
    that never happens, the helper goes back to its agent after 15 minutes,
    and the agent is told. Its row cannot be deleted until it has opened or
    the helper is back with its agent; the Delete dialog says so, and should
    the core still hold it, the deleted row comes back. Just after Luna
    starts or finds the core again, any chat's Delete waits a moment for that
    core's list of helpers, which Luna keeps asking for until it has it.
    While the core is not running, the Delete dialog says so and deletes
    nothing.
  - **Stop** and **Delete**; Delete waits while it is at work.
- Each helper's line says which of your other chats is waiting on it
  (`watched by <chat>`) and who manages it: `managed by chat <top>`
  last for a chat's own helper, `managed by <parent> · chat <top>` first for a
  helper's helper, `managed by <parent> · no chat above it` for one under a
  helper whose chat was deleted. A long line wraps, and the helper's read-only
  view says the same above its conversation.
- Each of your chats has a number of places for the sessions at work under
  it, its helpers' helpers included: 16 unless you change it in Settings →
  Agents & trust → Helpers (1 to 64). Only a helper at work takes a place: a
  turn running or waiting on a question, and a spawn under way. A helper
  between turns takes none, even with a background command it left running,
  so a chat may keep more helpers than it has places. A spawn's result and
  `luna_session_list` say how many are in use (`places`: `inUse`, the helpers
  at work and spawns under way on the chat's line, of `of`, `underYou`, and
  `waiting`, the turns waiting for a place). A spawn past the limit is
  refused: the helper is told which of its own are at work, to wait for or
  stop, and your chat is told too, as only it can free the rest. Text for a
  helper between turns is never refused for want of a place: it is queued,
  and its turn starts as soon as one frees, in the order such turns came,
  across the chat's line. Luna's own notices to a lead (a helper's answer, a
  question one asks it) start its turn even when every place is taken, so a
  lead never waits on places its own helpers hold. A helper's helpers get no
  tools.
- Every session gets its id as `LUNA_SESSION_ID`, a helper its parent's as
  `LUNA_PARENT_ID`. A lead or helper is told of its role in its instructions
  ([the library](library.md#what-a-session-is-told-of-its-role)), and a note
  after its task says the task comes from another agent, not a person: after
  the first task on Claude Code, after each on Codex. A prompt opening with a
  slash command goes without the note.

## Spawning options

- `readOnly: true` makes a checker: it takes Edit, Write and NotebookEdit from
  Claude Code (whose Bash can still write: a tool list, not a sandbox) and puts
  Codex in `read-only`, or `read-only-net` if asked.
- `agent: "<name>"` runs Claude Code as an agent the project or account defines
  (`--agent`); Codex has no such thing and refuses it.
- `account` can be a list of one provider's accounts: the first signed in with
  every usage window under 95%, else the one with most room. `accountChoice`
  says which and why.
- There is no `worktree` argument: Luna does not create or remove worktrees, and
  a spawn that passes `worktree` is refused with a message saying to pass a
  folder. For isolation create a worktree yourself with `git worktree add`, pass
  it as `folder`, and remove it when you are done. A helper spawned without a
  `folder` from a chat that runs in a worktree works in that worktree.
- A `folder` that does not exist is refused at the spawn.
- The spawn reports what settings cannot: other live sessions in the same
  directory (`sameFolder`), whose files and git it shares. `luna_session_list` names the
  model a turn ran on, where the organization swapped the one asked for.
- A helper runs in the mode its spawn names, whatever mode the chat spawning
  it runs in, and never in or above Luna's data folder. What a mode or a
  sandbox lets through is the CLI's own: Luna does not rank or compare them.
- A spawn that names no mode passes none: the helper's CLI runs in what its
  own settings say for that account and folder, with `modeFrom: "cli"` in its
  `settings`. A Claude Code helper's shows the `defaultMode` Luna read
  (`dontAsk` included); a Codex helper's says `sandbox: "codex"`, Codex's own
  from its config, which Luna does not resolve. A Codex config Luna cannot
  read (one that does not parse, or names a mode Luna does not know) makes
  it `read-only`, passed. A Claude Code helper may also run in `auto`, as a
  chat may (Auto), or in `dontAsk`.
- A Codex helper's approval is `never`, `on-request` (`on-failure` is read as
  it) or `auto-review`. What an `on-request` helper asks comes to its agent as
  a question ([Questions and permissions](#questions-and-permissions)); `auto-review` has a reviewer model answer instead,
  in `workspace-write`, and refuses any other sandbox named.
- `constraints` is text the helper and every session
  it starts in turn are told, last among their instructions, at every start,
  and resume. A spawn adds its own to the ones its caller carries, word
  for word; none is ever taken off (4 KB a spawn, 16 KB in all). They show in
  the helper's settings (`luna_session_get`).
  They are instructions, not permissions: what a helper may do is still its
  mode's. A helper you open as your own chat leaves them behind.
- A helper started with Luna's tools is a **lead**, one without them a
  **helper**: each is told of its role as described in
  [the library](library.md#what-a-session-is-told-of-its-role), and the
  library's **For** column can give instructions and skills to leads or
  helpers alone.

## Talking to sessions

- **Rights:** an agent kills and deletes only sessions it spawned (and theirs).
  By default it lists, reads and sends to its own helpers only, though a
  person's chat may look up another session's row by its id (`ids`,
  `luna_session_get`). With `scope: "all"` a chat lists every other session
  with `luna_session_list` (filtered by window `group`, account, state or
  `q`), gets one whole with `luna_session_get` (folder, project folder, cwd,
  the CLI's session id and transcript path), reads or waits on any of them,
  and types into your
  other chats with `luna_session_send` and answers their models' questions
  with `luna_session_answer`, so you can tell one chat to pass word to
  another. This holds for every chat of yours, also one created without Luna
  tools: the box decides what a chat can do through Luna, not what it can be
  sent. Text sent, and an answer typed in as a message, opens
  `[From session <id> via Luna]`; an answer to a question in a dialog is
  pressed in as keys. Another chat's
  helpers take text only from the sessions above them. `luna_session_list`
  narrows to one group (`group: "mine"`) and to rows with some text (`q`), so
  chats in one group can settle something between themselves.
- **Your draft is safe.** Into a chat you are typing in, `luna_session_send` does not
  type: Luna holds the text and types it once you stop. Over an unsent draft in
  Claude Code it stashes the draft (Ctrl+S), sends, and the CLI puts the draft
  back, pastes and images included, unless a draft is stashed already. Codex
  (0.169) has no stash, so the text waits for the draft to be sent or cleared,
  and goes in then. A chat with a
  draft wears a **draft** tag with the count of messages held, and a balloon says
  how many wait; a minute's hold is announced on the desktop.
- `luna_session_send` to a background Claude Code helper mid-turn joins that turn at its
  next tool boundary (a long tool or subagent delays it), as does one to a lead
  whose CLI has answered and stays up for its own background subagents (see
  Notices); if the CLI finishes first, the text starts the next turn; if that
  fails, it returns to the sender.
  After a core crash Luna looks for it in the transcript rather than send twice.
  Text to a Codex helper mid-turn steers that turn: it reads it at its next
  step and answers within the same turn. Sent just as the turn ends, it starts
  the next one instead.
- `luna_session_send` with `interrupt: true` stops the helper's turn and the text
  becomes the next turn: in the agent's own helpers only. Its CLI is asked to
  stop the turn itself first (a Claude Code helper's as Esc stops it, a Codex
  helper's by its server's interrupt), and is killed only if it does not
  within a few seconds. `luna_session_stop` stops a turn the same way.
- A message longer than 8,000 characters reaches one of your chats as a file it
  is told to read; a helper takes any length on its stdin.
- Luna types into a terminal one thing at a time (notices, sends and answers), so
  nothing lands inside a paste or in the way of a draft coming back. A send that
  cannot get its turn in time is held like one a draft is in the way of.

## Questions and permissions

`state` says `working`, `waiting` (a permission), `question`, `idle` and so on,
and `ask` carries the question with its options or the command up for
permission: Claude Code's `AskUserQuestion` and permission prompts, Codex's
questions and approvals. `luna_session_answer` answers: an option's number or label, own
words, or `allow` true or false.

A Claude Code helper puts its questions and permission prompts to Luna instead
of a screen, over its own input and output, so its agent answers them, any of them, whatever mode either runs
in. Text that a CLI reads as a command (opening with `/` or `!`) an agent sends
only into its own helpers. Each has an `id` in `ask`, which
`luna_session_answer` takes as `askId`, so an answer cannot land on the next prompt; an
id it never had is refused as unknown, and without one an answer is taken only
when a single prompt is held. The command up for permission is shown whole, up
to 8 KB, and marked where it is cut (`ask.detailCut` counts what is left out);
that is one field of the call, the command, path or URL, so a file's new
content or a subagent's prompt is not in it. A notice quotes only its start,
marked where shortened, line breaks shown as `⏎`, and `luna_session_get` has
the rest. Where Claude Code says
so, `ask.source` tells whether the helper itself asks or one of its subagents
does. A Codex helper's approvals come the same way, as permissions, by the
approval it was spawned with (`never` asks nothing; none named, its account's
config decides; with `approval: auto-review` a reviewer model answers instead).
In
one of your own chats an agent may answer only a question its model asked
(`scope: "all"`); a permission there is yours.

## Getting the answer

- `luna_message_list` reads the transcript, not the screen. `luna_session_wait` returns the
  report by default, with how many other messages there are (`rest`) and a file holding them
  all; `reply: "all"` returns everything, `"none"` nothing. `luna_report_get` gives the report
  alone and, with `ack: true`, collects the answer so no notice follows. The report is the
  message the CLI marked as its turn's final answer, or, without that mark, the longest of the
  turn, which a read then marks (`reportGuessed`). Past 60,000 characters a read is cut:
  `rest` says how many messages it left out, and its cursor goes on from there.
- `luna_session_wait` takes several sessions (`ids`, `mode: "any"` or `"all"`) and says
  which finished; the terminal comes only with `screen: true`. `luna_session_list` gives
  compact rows a page at a time with filters; `luna_session_get` one session whole; accounts
  are `luna_account_list` and `luna_account_get` (defaults, usage, models).
- **Notices.** An agent that asked and ended its turn is not left idle: once the
  answer comes (or the helper asks something, stops on a permission, or exits),
  Luna types a one-line `[Luna notice, not typed by the user]` into the agent's
  chat, as soon as it is between turns and you have not typed into it for 20
  seconds. When another helper it is waiting on is mid-turn, the notice waits up
  to 20 seconds more for that one's, so helpers finishing together make one
  notice and one turn of the agent; a question or a permission prompt never
  waits. A draft you left there is stashed the same way (Claude Code) or holds
  the notice (Codex). A `luna_session_wait` still out for that answer hands it over
  instead, and the notice follows only if the wait has not within 10 seconds
  (Claude Code moves a call past two minutes to the background). A wait the
  agent's CLI cancels or interrupts hands nothing over, and the notice comes as
  usual. A subagent the agent left working in the background does not hold the
  notice back. That goes for a Claude Code lead too, whose CLI stays up past
  its answer while its own background subagents run: the notice goes in on its
  stdin and it answers it at once, or right after a turn its CLI is at (one it
  starts by itself when a subagent ends), for up to 3 hours after that answer
  (later ones wait for its CLI to end). A Codex lead's process ends with its
  turn, its subagents with it, so nothing holds its notices.
- A CLI's own background subagents live inside the helper's turn and end
  with it. When a Claude Code helper's turn ends with some of them still at
  work (its CLI killed, gone on an error, or out of time to wait for them),
  the agent's notice, and its wait or report, says so by name: "2 background
  subagents ("Implement slice A", "Implement slice B") ended with its turn
  without finishing; their work is not done", never that they finished, and
  that a send has the helper check what they left and carry on. When a
  Codex helper's turn ends with its subagents unfinished, the
  notice says that too. When the turn was stopped (Stop, an interrupt, an
  update), it says only that they were cut. Only subagents the helper
  started itself are named; theirs end with them.
  Work that must outlive a turn belongs in Luna helpers.
- The notice says what happened, how many messages the helper wrote, the first
  line of its report, and a file with all of them: the report (the longest
  message, or the last when all are short — but a short turn the CLI started by
  itself, on a background job's end, leaves the report to the long message
  before it) first, then the last, then the rest.
  A `luna_message_list` collects the answer only when it returns all of it, uncut, from a
  finished helper.
- Text sent to one of your chats mid-turn may join that turn or start the next,
  so Luna gives the CLI 8 seconds after a turn ends before taking it as the
  answer.
- A turn that stopped on an error (a usage limit, an API error, a crash, a kill
  not of Luna's) is reported as that, with the reason; `luna_session_wait` has it
  as `failed`. One Luna stopped (`luna_session_stop`, Stop, a send with
  `interrupt`) is no error: the notice says it did not finish, and the wait has
  the reason as `stopped`. What the
  helper said before it stopped comes with it. A helper whose CLI quit before its
  model did anything (an unknown `agent`, a model that is not there) is idle, not
  busy, and goes like any other.
- A helper that ended its turn with a job running, or helpers of its own still
  out, is announced as not done once; the next notice comes when that is over or
  it ends a turn of its own (a turn it only ran on a Luna notice does not count,
  unless that was its own helper's last awaited answer), so two chats with a
  stray process each do not keep waking each other. A background Claude Code
  helper's own `run_in_background` job keeps its turn going: it stays `working`
  until the job ends, and there is one notice with everything it said. What a helper makes of its helpers' answers reaches
  its parent when it is 400 characters or more, once, only upward.
- A job's end the helper says nothing about rides with the next notice, alone
  only once nothing else is owed, nothing is at work, or after 10 minutes.
- What turns left running (a server, a watcher, a forgotten `tail -F`) is named in
  the notice and in `luna_session_get` as `leftRunning` (by name and pid, or a
  count of shells). It keeps the helper, and the PC, active for as long as it is
  there, however idle. For a background helper `luna_session_stop`, or Stop in the
  helper list, ends it once the turn is over; the notice of its end comes when it
  ends. A turn silent for 30
  minutes gets one notice too (a command may hang); a command with its own
  timeout holds that until 5 minutes past it.
- **Breaker:** past 12 turn-end notices about one session in 15 minutes, Luna
  stops telling that chat of that session's turns and jobs, says so in the log
  and once on the desktop, and resumes when you type into either chat. Prompts
  and questions still come.
- An agent also gets a notice when an account its helpers run on reaches 90%
  and then 95% of a usage window, once each per window.

`luna_session_list` shows per helper its state, the first line of its task and when it last
did anything; `luna_session_get` adds its transcript and the CLI's own conversation id,
when its turn began, the model it ran on (none until a model has answered), how full its
context is after its last turn and the version of the CLI its process last started from
(`cliVersion`).
`luna_account_list` names the accounts agents may use (the account alone is enough: it brings
its own CLI) with their usage as the Accounts window shows it; `luna_account_get` adds the
settings a helper would get there (read from the account's and the project's settings the way
the new-chat dialog reads them) and its models.

## Lifetime

- A helper goes by itself (`autoDelete`, on by default) once its answer reached
  the agent and it has sat idle 30 minutes with nobody reading or writing to it;
  `luna_session_list` shows `deletesInMin`. It waits for jobs it left, however idle,
  and helpers of its own.
- A helper stopped with `luna_session_stop` is never auto-deleted; one you opened as a
  chat is yours and stays.
- `luna_session_delete` removes the helper and nothing of a worktree: a
  `dropWorktree` argument is refused, saying Luna removes none, and a worktree the
  agent made for the helper is the agent's to remove.
- Deleting a chat by hand lists its helpers with what each is doing, and leaves them alone unless you tick the box to take them
  along. Left alone, each of the chat's own helpers answers to no chat from
  then on and deletes itself half an hour after it is done and no helper of
  its own is left; the helpers under it stay under it, and go only when
  deleted or, if they were started to delete themselves, once done. With the
  box ticked, a helper still at work (in a turn, on a question, a job it left
  running) is stopped only if you also tick that; Luna checks again as
  it deletes. Only the helpers the dialog listed are deleted. Helpers an
  agent deletes are gone (transcript aside); what they
  spawned passes to that agent. A helper with no parent is listed under
  "Helpers with no chat" and goes half an hour after it is done.
- Every spawn is recorded in `agents.jsonl` (when, by whom, where, account,
  settings, the start of the prompt, how it ended), so `luna_run_list` shows what ran
  before, and a run can be repeated. A deleted
  helper leaves its transcript path and the first 400 characters of its
  last message.

## Codex sandbox notes

What a sandbox holds is Codex's own, not Luna's; these are what was seen of
it (Codex 0.169.0, [CLI compatibility](../cli-compatibility.md)).

- Codex under `read-only`, `read-only-net` or `workspace-write` (so also
  `auto-review`) runs commands in its Windows sandbox, without the Store's app
  aliases on its PATH. Where the account names no `windows.sandbox`, Luna
  passes the unelevated one (no administrator, no extra accounts), as Codex
  runs no command in those sandboxes without one; one the account names is
  left as it is.
- `read-only-net` and `danger-full-access` have the network. `read-only` and
  `workspace-write` do not by default, but on Windows Codex only points proxy
  variables at a dead port, so ssh and raw sockets still connect: that keeps
  tools off the network by accident, not data in. `read-only-net` is Luna's
  value (Codex's read-only with network, passed per run, never written to
  `config.toml`); `auto-review` cannot use it.
- On Windows, schannel TLS (built-in `curl`, `Invoke-WebRequest`) and Git's
  bundled ssh fail under the sandbox; `gh`, Python, plain http and git over https
  work (Luna sets git's `http.sslBackend=openssl` there unless you chose one).
  For git over ssh set `GIT_SSH` to Windows' own `ssh.exe`.
- Luna's `core` folder (in the data folder) holds nothing that lets anyone
  in: `core.json` names the running core's process and the pipe or socket it
  listens on, beside the lock that keeps the core single and, on Windows, the
  copies of the exe the core runs from. The core lets in only your processes
  the OS does not mark as confined, so Codex's sandbox on Windows and Linux is
  refused whatever it can read. Sessions are still kept out of the folder
  where the CLI allows it, as a second wall (off Windows the core's socket is
  there): off Windows, `read-only` and `read-only-net` sessions cannot read
  it. `workspace-write` sessions, and every sandbox on Windows, still can:
  Codex offers no way to deny them a folder that keeps your other settings for
  those sandboxes. Claude Code's file tools are denied it everywhere; its Bash
  is not, and like any program of yours that runs unconfined, it can reach the
  core.
