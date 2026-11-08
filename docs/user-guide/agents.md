# Agents: sessions that run sessions

A chat created with **Luna tools** ticked (off by default) gets Luna as an MCP
server, and with it twelve tools: `luna_list`, `luna_sessions`, `luna_spawn`,
`luna_send`, `luna_answer`, `luna_read`, `luna_wait`, `luna_kill`,
`luna_delete`, `luna_guide`, `luna_notify`, `luna_power`.

The model can start a helper session (another model, another account, Codex
instead of Claude) with an opening prompt, wait for its reply, send follow-ups,
answer what it asks, and delete it when done, worktree and its `worktree-*`
branch included if it asked for one. The tools and a short instruction cost
about 2k tokens once per session; the full manual is `luna_guide`'s, read only
when the model asks: whole with `topic: "all"` (the instruction says to, before
the first spawn), one section by a word of its title (`topic: "spawning"`), or,
with no topic, its opening and the sections to ask for. A chat without the box knows nothing of Luna, and neither
does a background helper spawned without tools.

- `luna_notify` raises a desktop notification mid-turn, titled with the chat's
  name and opening it when clicked, at most one every 10 seconds.
- `luna_power`, for when you ask for it, arms the same "when done" shutdown,
  restart, hibernate or sleep as the power chip, which shows and cancels it; the
  action `cancel` disarms it too.
- A spawn may set `budgetTokens`: once that helper's context passes it, its
  parent hears so, once; nothing is stopped.
- An agent reads, waits on, sends text to and answers its own helpers. Your
  own chats, another chat's helpers and its own parent it reaches only by
  saying so (`scope: "all"`), and what it sends them, an answer typed in as a
  message too, arrives opened `[From session <id> via Luna]`, so that chat
  knows the words are not yours. Only its own helpers
  can it interrupt, stop or delete.
- A helper starts without its account's MCP servers and connectors (claude.ai's
  Gmail, Slack and the like; Codex's apps and the servers in its account's
  `config.toml`), and a Claude Code helper with auto-memory off, so that what
  an agent briefs it to do stays inside Luna. A spawn with `connectors: true`
  keeps them. A Codex helper still has servers from the project's `.codex/`
  folder and from plugins.
- A tool called with an argument it does not take, or of the wrong type, is
  refused with what it does take, rather than run on a default.

**The guard.** A chat with tools, and every helper, has its shell commands
checked before they run: one that follows a file (`tail -f`, `tail -F`,
`Get-Content -Wait`, in Bash, PowerShell or a Monitor) with no `timeout N` in
front is refused, with what to do instead. Such a wait outlives the tool that ran
it and holds the PC awake all night. Codex sessions get this only where Codex
hooks run unasked.

## Helpers

- Every helper runs in the **background**: no row, no pane, no terminal. Each
  turn is one run of the CLI's headless mode (`claude -p`, `codex exec`); the
  next resumes the conversation. It needs no open window. Its parent's row and
  pane title bar show how many there are (the count glows a few seconds on a
  spawn); a click lists them with what each is doing and for how long, its
  model, effort, account and worktree, and offers:
  - **Watch:** its conversation, read-only, as it goes: what its agent asked and
    what it said, tool calls marked, updating every second while a turn runs.
    Nothing typed there reaches it.
  - **Open as chat** (between turns, once nothing it left runs): it becomes a
    chat of your own, for good, a row in its folder seated in a pane, going on
    with the same conversation. Its agent loses it: it can no longer send to it,
    wait on it, read it as its own, stop or delete it, and is owed nothing more.
    The agent hears so once, in a notice that survives a restart. Text still
    queued for the helper is not delivered: whoever sent it is told, and the
    notice names the file it is in. Luna asks before it does this.
  - **Stop** and **Delete**; Delete waits while it is at work, and keeps a
    worktree that holds uncommitted work.
- At most eight helpers under one chat, and a helper's helpers get no tools.
- Every session gets its id as `LUNA_SESSION_ID`, a helper its parent's as
  `LUNA_PARENT_ID`. A helper is told what it is — not the chat of whoever its
  instructions call the operator, the user or the person at this machine — and
  to write in the language of its task: Claude Code through its system prompt
  and a note after its first prompt; Codex, which can only replace
  `developer_instructions`, through a note Luna appends to each prompt. A prompt
  opening with a slash command goes without it.

## Spawning options

- `readOnly: true` makes a checker: it takes Edit, Write and NotebookEdit from
  Claude Code (whose Bash can still write: a tool list, not a sandbox) and puts
  Codex in `read-only`, or `read-only-net` if asked.
- `agent: "<name>"` runs Claude Code as an agent the project or account defines
  (`--agent`); Codex has no such thing and refuses it.
- `account` can be a list of one provider's accounts: the first signed in with
  every usage window under 95%, else the one with most room. `accountChoice`
  says which and why.
- `worktree: true` keeps the helper apart; the spawn names the worktree the CLI
  made and its branch (`luna_list` shows them for every helper).
- The spawn reports what settings cannot: other live sessions in the same
  directory (`sameFolder`), whose files and git it shares. `luna_list` names the
  model a turn ran on, where the organization swapped the one asked for.
- A helper runs at no more privilege than its caller (permission mode; Codex
  sandbox, approval and network) unless Settings → Helpers allows helpers above
  their caller, and never in or above Luna's data folder. Turning that setting
  on asks again in a system dialog; turning it off does not.
- A Codex helper never asks a person: its approval is `never` or `auto-review`,
  and one that asks (`on-request` and the like) is refused, as nobody is there.

## Talking to sessions

- **Rights:** an agent kills and deletes only sessions it spawned (and theirs).
  It can see every other one, yours included, with `luna_sessions` (name, window
  group, account and folder, project folder, cwd, the CLI's session id and
  transcript path), type into any of them with `luna_send`, and read or wait on
  the answer, so you can tell one chat to pass word to another. `luna_sessions`
  narrows to one group (`group: "mine"`) and to rows with some text (`q`), so
  chats in one group can settle something between themselves.
- **Your draft is safe.** Into a chat you are typing in, `luna_send` does not
  type: Luna holds the text and types it once you stop. Over an unsent draft in
  Claude Code it stashes the draft (Ctrl+S), sends, and the CLI puts the draft
  back, pastes and images included, unless a draft is stashed already. Codex has
  no stash, so the text waits for the draft to be sent or cleared. A chat with a
  draft wears a **draft** tag with the count of messages held, and a balloon says
  how many wait; a minute's hold is announced on the desktop.
- `luna_send` to a background Claude Code helper mid-turn joins that turn at its
  next tool boundary (a long tool or subagent delays it); if the CLI finishes
  first, the text starts the next turn; if that fails, it returns to the sender.
  After a core crash Luna looks for it in the transcript rather than send twice.
  Codex's `exec` reads its input once, so text to a background Codex helper waits
  for the next turn.
- `luna_send` with `interrupt: true` stops the helper's turn and the text
  becomes the next turn: in the agent's own helpers only.
- A message longer than 8,000 characters reaches one of your chats as a file it
  is told to read; a helper takes any length on its stdin.
- Luna types into a terminal one thing at a time (notices, sends and answers), so
  nothing lands inside a paste or in the way of a draft coming back. A send that
  cannot get its turn in time is held like one a draft is in the way of.

## Questions and permissions

`state` says `working`, `waiting` (a permission), `question`, `idle` and so on,
and `ask` carries the question with its options or the command up for
permission: Claude Code's `AskUserQuestion` and permission prompts, Codex's
questions and approvals. `luna_answer` answers: an option's number or label, own
words, or `allow` true or false.

A Claude Code helper puts its questions and permission prompts to Luna instead
of a screen, so its agent answers them. Each has an `id` in `ask`, which
`luna_answer` takes as `askId`, so an answer cannot land on the next prompt; an
id it never had is refused as unknown, and without one an answer is taken only
when a single prompt is held. A Codex helper has nobody to ask and says what it
would have asked in its reply, unless it runs with `approval: auto-review`. In
one of your own chats an agent may answer only a question its model asked
(`scope: "all"`); a permission there is yours.

## Getting the answer

- `luna_read` reads the transcript, not the screen; `luna_wait` returns the reply
  itself, everything the helper said since it was asked, report first. Past
  60,000 characters either is cut: `luna_read` says how many messages it left
  out (`leftOut`), goes on from its cursor with `since`, keeps the newest with
  `last`; `luna_wait` names a file with the whole answer. `luna_read` with
  `report: true` gives the report alone (of what was said since `since`, else
  since the agent asked), and does not count as collecting the answer. A notice
  says which lines of its file are the report (`lines 1-N of M`).
- `luna_wait` takes several sessions (`ids`, `mode: "any"` or `"all"`) and says
  which finished. `screen: false` leaves the terminal out; `luna_list` with `ids`
  gives those rows alone, and `accounts: false` leaves the accounts out. The
  accounts' models (Codex's catalog with each model's levels) come only with
  `models: true`.
- **Notices.** An agent that asked and ended its turn is not left idle: once the
  answer comes (or the helper asks something, stops on a permission, or exits),
  Luna types a one-line `[Luna notice, not typed by the user]` into the agent's
  chat, as soon as it is between turns and you have not typed into it for 20
  seconds. When another helper it is waiting on is mid-turn, the notice waits up
  to 20 seconds more for that one's, so helpers finishing together make one
  notice and one turn of the agent; a question or a permission prompt never
  waits. A draft you left there is stashed the same way (Claude Code) or holds
  the notice (Codex). A `luna_wait` still out for that answer hands it over
  instead, and the notice follows only if the wait has not within 10 seconds
  (Claude Code moves a call past two minutes to the background). A wait the
  agent's CLI cancels or interrupts hands nothing over, and the notice comes as
  usual. A subagent the agent left working in the background does not hold the
  notice back.
- The notice says what happened, how many messages the helper wrote, the first
  line of its report, and a file with all of them: the report (the longest
  message, or the last when all are short — but a short turn the CLI started by
  itself, on a background job's end, leaves the report to the long message
  before it) first, then the last, then the rest.
  A `luna_read` collects the answer only when it returns all of it, uncut, from a
  finished helper.
- Text sent to one of your chats mid-turn may join that turn or start the next,
  so Luna gives the CLI 8 seconds after a turn ends before taking it as the
  answer.
- A turn that stopped on an error (a usage limit, an API error, a crash, a kill)
  is reported as that, with the reason; `luna_wait` has it as `failed`. What the
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
- A job's end the helper says nothing about, or a job gone idle, rides with the
  next notice, alone only once nothing else is owed, nothing is at work, or after
  10 minutes.
- What turns left running (a server, a watcher, a forgotten `tail -F`) is named in
  the notice and in `luna_list` as `leftRunning` (pid, start, recent CPU or I/O).
  For a background helper `luna_kill`, or Stop in the helper list, ends it once
  the turn is over. All idle for 30 minutes: the owed agent gets one notice that
  it may be stuck or waiting, and how to end it; the notice of its end still
  comes. A turn silent for 30
  minutes gets one notice too (a command may hang); a command with its own
  timeout holds that until 5 minutes past it.
- **Breaker:** past 12 turn-end notices about one session in 15 minutes, Luna
  stops telling that chat of that session's turns and jobs, says so in the log
  and once on the desktop, and resumes when you type into either chat. Prompts
  and questions still come.
- An agent also gets a notice when an account its helpers run on reaches 90%
  and then 95% of a usage window, once each per window.

`luna_list` shows per helper: the first line of its task, branch and commit, when
its turn began and last did anything, the model it ran on (none until a model has
answered), and how full its context is after its last turn. It names the accounts
agents may use (the account alone is enough: it brings its own CLI), each with
its models when asked (`models: true`), the settings a helper would get there (read from the account's and the
project's settings the way the new-chat dialog reads them) and its usage as the
Accounts window shows it.

## Lifetime

- A helper goes by itself (`autoDelete`, on by default) once its answer reached
  the agent and it has sat idle 30 minutes with nobody reading or writing to it;
  `luna_list` shows `deletesInMin`. It waits for jobs it left and helpers of its
  own; processes idle 2 hours on end do not count and go with it.
- A helper stopped with `luna_kill` is never auto-deleted; one you opened as a
  chat is yours and stays, as does a worktree it made.
- `luna_delete` with `dropWorktree` takes the worktree the CLI made and its
  `worktree-*` branch (a branch the helper named itself stays). It refuses when
  that would lose uncommitted changes, untracked files or unique commits, and
  says which, unless `force: true`. A worktree that cannot be removed comes back
  as `worktreeLeft`. Your own Delete asks you instead.
- Deleting a chat by hand lists its helpers with what each is doing and what its
  worktree holds, and leaves them alone unless you tick the box to take them
  along. Even then a helper still at work (in a turn, on a question, a job it
  left running) is stopped only if you also tick that, and a worktree with
  uncommitted work goes only if you tick that too; Luna checks both again as it
  deletes. Helpers an agent deletes are gone (transcript aside); what they
  spawned passes to that agent. A helper with no parent is listed under
  "Helpers with no chat" and goes half an hour after it is done.
- Every spawn is recorded in `agents.jsonl` (when, by whom, where, account,
  settings, the start of the prompt, how it ended), so `luna_sessions` with
  `history: true` shows what ran before, and a run can be repeated. A deleted
  helper leaves its transcript path, worktree and the first 400 characters of its
  last message.

## Codex sandbox notes

- Codex under `read-only`, `read-only-net` or `workspace-write` (so also
  `auto-review`) runs commands in its Windows sandbox (unelevated: no
  administrator, no extra accounts), without the Store's app aliases on its
  PATH.
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
- Luna's `core` folder (in the data folder) holds the key to the whole app.
  Off Windows, `read-only` and `read-only-net` sessions cannot read it.
  `workspace-write` sessions, and every sandbox on Windows, still can: Codex
  offers no way to deny them a folder that keeps your other settings for those
  sandboxes. Claude Code's file tools are denied it everywhere; its Bash is not.
