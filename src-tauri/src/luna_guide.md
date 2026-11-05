# Luna: sessions, accounts and the luna_* tools

Luna is the desktop app this session runs in. It runs Claude Code and Codex sessions side by
side, each on one of the user's accounts, and through these tools any session that has them
can start other sessions and work with them.

## Sessions
- Every chat in Luna is a session: the user's own chats and the helpers sessions start. Each
  has an id, a name, a provider (`claude` = Claude Code, `codex` = Codex), an account, a
  project folder and a state: `working`, `waiting` (a permission prompt), `question`,
  `idle`, `starting` (its CLI has not drawn yet), `background` (a job, a subagent its CLI
  runs in the background, or a helper of its still runs), `exited`.
- A helper is **background** by default: no row, no terminal; each turn is one run of the
  CLI's headless mode (`claude -p`, `codex exec`) on the helper's account, and the next turn
  resumes its conversation. The user sees it as a count on your chat's row and can open it as
  a chat between its turns. `visible: true` makes it a normal chat row instead, nested under
  yours, that the user can type into.
- Each `luna_list` row also says:
  - `turnStartedAt`: when the turn on now began; null between turns. `lastActivityAt`: its
    last event (a visible session's last transcript write); a long command or think shows
    nothing until it ends, so quiet is not stuck. Times are UTC ISO, as in `luna_sessions`
    history.
  - `resolvedModel`: the full model id a model answered on; null until one has (Codex: what
    its rollout says the turn ran on, or the `<to>` of a `model rerouted` message).
  - `context`: how full its context window is after its last turn: `tokens` in the window
    (its input, cache included) against the model's `window`, and `pct` of it (0-100). Left
    out until a turn has run; during one, as of its latest step.
  - `leftRunning`: what its turns left running, between turns: `exe`, `pid`, `startedAt`,
    `active` (CPU or I/O in the last 10 min), `quietMin`. It keeps the PC awake until idle 2 h
    on end. `luna_kill` ends a background helper's alone (the user's Stop too). Killing a
    turn stops its CLI; what outlives that (on Windows a Git Bash command's own processes, a
    `run_in_background` job's) shows here after, for a second `luna_kill` to end.
- You kill and delete only sessions you spawned and theirs. Any other session you can read,
  wait on and send text to. A deleted parent's helper passes to its parent; with none, only
  the user can stop or delete it, and a background one goes once idle for 30 min.
- At most eight helpers under one chat. A helper given `tools: true` gets these tools too,
  but its own helpers get none.
- `LUNA_SESSION_ID` holds a session's id, `LUNA_PARENT_ID` a helper's parent's. A Claude Code
  helper's system prompt says it is one; a background Codex helper's every prompt ends with
  Luna's note saying so, a visible one's first (not after a `/command`).

## Accounts and models
- An account is a signed-in login of one provider: an Anthropic account runs Claude Code
  only, an OpenAI account runs Codex only. `provider` and `account` in `luna_spawn` must
  match; an `account` alone brings its own provider. `luna_list` has only those the user lets
  agents use.
- `account` as a list, of one provider: the first signed in with every usage window under 95%
  or unknown, else the most room. `accountChoice` says which and why.
- Accounts of one provider differ: personal or team plans, different organizations, their
  own allowed models and their own quota. `defaults` in `luna_list` come from the account's
  and the project's settings. A usage window near 100% means a helper there may stop on its
  limit. When an account your live helpers run on reaches 90%, then 95%, of a window, a
  notice says so, once each until it resets (by the figures Luna's window keeps: none open,
  no notice).
- A model the organization does not allow, swapped by the CLI, is in the spawn's `warnings`.

## Spawning
- Codex's `approval`: auto-review has a reviewer model answer what Codex would ask
  (workspace-write only); on-request is for visible ones: a background Codex has nobody to
  ask, and runs with never unless told auto-review, the sandbox being what holds it. Running
  the project's hooks past Codex's trust prompt is the user's setting per folder: without it
  a background Codex skips untrusted hooks, a visible one stops on it. A background Codex trusts the
  folder it is sent to for its run, so the project's own `.codex/` config loads; a visible
  one in a folder Codex does not trust yet has Luna trust it for that account.
- Codex's network: none in `read-only` or, unless its config says so, `workspace-write`, but
  on Windows only by dead proxy variables: ssh and raw sockets still connect.
  `danger-full-access`: the machine's. `read-only-net` reads only, with network; on Windows
  schannel TLS fails (built-in `curl`, `Invoke-WebRequest`), as does git's own ssh; `gh`, Python,
  plain http work, and git https (Luna sets `http.sslBackend=openssl`).
- Codex's `workspace-write` keeps `.git` read-only, its own rule: such a helper cannot commit,
  in a worktree or not. One that must commit needs `danger-full-access`.
- `readOnly`: Claude Code's **Bash can still write**; Codex's sandbox (`read-only`, or
  `read-only-net` if given) holds. Refused with `acceptEdits`, `auto-review` or a sandbox that
  writes.
- `agent`: an unknown name stops the CLI. Codex refuses it: its roles are for subagents only.
- `worktree: true` shares no files or git with the sessions in that folder; the result
  names the worktree and branch, or `worktreePending` until the CLI has made it. `sameFolder`
  lists live sessions working in the same directory.
- `started`: a background helper's process is up; a visible one was seen on its prompt within
  30 s, else `screen` shows why (a dialog: `ask` too).
- A prompt over 8,000 characters reaches a visible session as a file it is told to read.

## Getting the answer
- After a spawn or a `luna_send` you can end your turn. Once that session answers, asks
  something, stops on a prompt or exits, Luna types one line into your chat starting with
  `[Luna notice, not typed by the user]`. It says what happened and how many messages the
  session wrote since it was asked, and names a file holding them: the report (the longest
  message of its last turn that wrote any; if all of those are short, the last) first, then
  the last message, then the rest in order, earlier turns' reports among them.
  The notice quotes only the report's first line, cut to about 150 characters (`It begins:
  "…"`), and not of a turn that stopped on an error, which it says.
  It comes when your chat is between turns and the user is not typing in it; a subagent you
  left working in the background does not hold it back.
- "NOT done yet": a job it started still runs (named), a subagent of a visible one's CLI
  works on in the background, or its own helpers have yet to answer it. Said once; the next
  notice comes when that is over or a turn of its own ends (all of it
  idle 30 min: one more says so; it may be stuck or waiting), not one it only ran
  on a Luna notice: unless that was its own helper's last answer, or, to you its parent, one on
  its helpers' notices that wrote a message of 400+ characters. A background Claude Code
  helper's own `run_in_background` job keeps its turn going: `working` until it ends, one notice.
- Past 12 turn-end notices about one session in 15 minutes Luna stops telling you of its turns
  and jobs (the last says so; its prompts and questions still come) until a person types into
  either chat; `luna_wait` and `luna_read` still work.
- Use `luna_wait` when you need the answer within the same turn. `outcome` says which: `turn_done`,
  `waiting`, `exit`, `job_done` or `timeout`. On `turn_done` (and an `exit` after such a
  turn), `reply` holds all the session said since you asked, the report first. `failed` names the error a turn stopped on (API error, usage limit,
  crash, kill): `reply` is what it said before that, if anything, and `screen` comes too.
  `transcriptMissing`: no transcript found, so a null `reply` says nothing. `job_done`: told
  it is not done, what it had running ended with no turn; `reply` is what it said since. A
  background helper does not exit between turns: `until: "exit"` returns once it is deleted.
- `luna_wait` with `ids: [..]` waits on several: `mode: "any"` until the first gets there,
  `"all"` until every one has or the time is up. It returns `outcome` (`finished` or
  `timeout`), `finished` (the ids that got there) and `results`, one per id in the shape a
  single wait returns, with `id`. Under `any` the others come back `pending`, or `timeout`,
  and are still owed. Their replies share one result: past 60,000 characters in all the
  longest are cut, and each names a file with the whole.
- A `luna_wait` running when the answer comes hands it over instead of the notice, one your
  CLI moved to the background too. One you cancel or interrupt hands nothing over (one on
  several: none of them), and the notice comes as usual. Claude Code hands a background one's
  result over HTML-escaped (`&lt;`, `&gt;`): take code from `luna_read`, not from that.
- A turn that has written nothing for 30 min is told of once ("mid-turn and has written
  nothing"), and not before 5 min past the timeout of a command it waits on: a hung command,
  or a long one. `luna_read` with `screen: true` tells which.
- Without `afterTurn`, a wait is for the answer you are still owed, else for a turn past the
  last count you were shown.
- `luna_read` with `since` and `last` gives the last N after the cursor, which goes past all
  of them. `tools: true` is Claude Code only; `screen: true` gives a background helper's last
  log lines. A message past 60,000 characters is cut; `luna_sessions`' transcript has it whole.
- A read of all a session said since you asked, whole, after it finished, collects the answer:
  no notice follows. Not one of only the last of several, or from a cursor taken since.
- Asked again before an answer reached you, the next reply, read or notice covers all since the
  first question.
- A core restart stops a background helper's turn and what its turns left running (off Windows
  only if it died). You stay owed: once your chat is back (given up after 10 minutes) a notice
  says it did NOT finish; `luna_send` has it carry on. What you queued never reached it: the
  notice counts it and its file ends on it (`undelivered` in `luna_read`,
  `luna_wait`). A helper stopped with no conversation yet, visible too, "never got started": the
  file holds its prompt, then that text; `luna_send` starts it afresh, or with its CLI not back
  spawn it again. A worktree it began is reused only if Luna named it and untouched; else `worktreeLeft`.
- `resumeOnRestart` goes on by itself with that text (afresh on its prompt if it never got started);
  one notice says so, the usual one follows. Else the plain notice: a background helper where Luna
  cannot tell the old turn ended, a visible one whose CLI Luna could not bring back, or that the
  user or you wrote to first.
- Visible sessions and chats you wrote to come back when the window starts them, with text held for
  them; the notice comes then, or after a minute saying they have not.
- A `luna_spawn` a restart cut off errors. Wait for the notice before spawning again: no helper came
  of it (a file has its prompt; any `worktreeLeft`), or with `resumeOnRestart` it started afresh.

## Answering and steering
- A session in state `question` or `waiting` has an `ask`: the questions with their
  options, or the tool up for permission. In `luna_answer` an option's number counts from 1.
  It returns `state`, `ask` and the `screen` after it. A background helper's `ask` has an
  `id`: pass it as `askId`, so your answer cannot land on the next one; a stale or unknown id
  is refused, and so is no id with several held.
- A visible Codex stopped on a list of its own ("Hooks need review" at startup) has it as a
  question too, its options the only answers it takes; a spawn it kept back returns it as `ask`.
- A background Claude Code helper's questions and permission prompts come to you this way, one
  at a time. A background Codex helper's are in its reply.
- `luna_send` sent while a session works is `queued`: a visible session's CLI takes it into
  the running turn or the next, a background helper always the next; `luna_wait` with the
  `turnsEnded` returned waits for whichever answers it. `interrupt: true` makes the text the
  next turn, behind any queued (`queued` if that turn would not stop). Where the user is
  typing, or has left an unsent draft Luna cannot put aside (Claude Code's is put aside and
  comes back; Codex's cannot be), the result says `held: true`: Luna types it in once they
  are done, and a `luna_wait` waits for that first. Do not send it again.
- `autoDelete`: Luna deletes the helper 30 minutes after its answer reached you, not while a
  job or helper of its runs (processes idle 2 h on end do not count: they go with it, and you
  hear so); a read or a send starts that over (`deletesInMin` counts down). The worktree stays. Off
  for visible ones unless `true`. One the user typed into, or whose turn you killed, is never
  deleted; ending only what it left running between turns keeps it as it was.
- A deleted session's transcript stays in its account's folder (`luna_sessions` history names
  it). If a helper printed what it should not have, tell the user that path.

## Working with helpers
- A helper acts as you. Messages, posts, emails, pushes or writes elsewhere in the user's name
  go out only when the user asked for them, by you or by a helper you brief.
- Relaying a report, keep its caveats ("possibly a false flag", "not tested live") with the
  claim. Ask reviewers and implementers to say which findings they think wrong, and why.
- Before ending a turn on "a notice will come", for anything long or overnight: a `luna_wait`
  with a timeout, or check `luna_list` when the user is back. "NOT done yet" comes once.
- Before anything that stops Luna's core or the PC: `luna_list` for helpers at work; they lose
  their turns. To turn the PC off when work is done use `luna_power`, not a `shutdown` command:
  it waits for every session and closes the CLIs first.
