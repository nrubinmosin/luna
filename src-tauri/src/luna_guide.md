# Luna: sessions, accounts and the luna_* tools

Luna is the desktop app this session runs in: Claude Code and Codex sessions side by side, each
on one of the user's accounts. Each section below answers the topics `luna_guide_get` lists.

## Sessions
- Every chat in Luna is a session: the user's own chats and the helpers sessions start. Each
  has an id, a name, a provider (`claude` = Claude Code, `codex` = Codex), an account, a
  project folder and a state: `working`, `waiting` (a permission prompt), `question`,
  `idle`, `starting` (its CLI has not drawn yet), `background` (a job, a subagent its CLI
  runs in the background, or a helper of its still runs), `exited`.
- Every helper runs in the background: no row, no terminal; each turn is one run of the CLI's
  headless mode (`claude -p`, `codex exec`) on the helper's account, and the next turn resumes
  its conversation. The user sees it as a count on your chat's row, can watch it read-only, and
  can open it as a chat between its turns. Opening hands it to the user for good: no longer yours
  to send to, wait on, read, stop or delete; nothing more is owed by it; text still queued for it
  is not delivered; one notice tells you so, and where that text is.
- `luna_session_list` gives compact rows, yours by default (`scope: "all"`: every session;
  `ids`: those): `id`, `name`, `provider`, `account`, `parent`, `state`, `askId`, `turnsEnded`,
  `task` (its prompt's first line), `lastActivityAt`, `deletesInMin`. Filters: `state` (a list),
  `provider`, `account`, `group` (a window group, I = 1, or `"mine"`), `q` (text in name, folder,
  account or id). `limit` rows a page (50); a `nextCursor` goes in `cursor` for the next.
- `luna_session_get` gives one session whole: `settings`, `folder`, `cwd` (its worktree),
  `transcript`, `sessionId` (the CLI's own), `worktree`, `branch` and `sha` (short; no branch: detached, as a Codex worktree
  is), `turnStartedAt` (null between turns), `lastActivityAt` (a long command shows nothing
  until it ends: quiet is not stuck), `resolvedModel` (the model that answered), `context`
  (`tokens` in its window against the model's `window`, `pct`, after its last step) and
  `leftRunning`: what its turns left running between turns (`exe`, `pid`, `startedAt`,
  `active` = CPU or I/O in the last 10 min, `quietMin`), which keeps the PC awake until idle 2 h.
  Times are UTC ISO. A helper in a worktree adds `lastCommit` (its subject) and `status` (`git
  status --short`, 20 lines, `statusMore` the rest): your CLI may keep you out of that worktree.
- Another session's `task`, `askId` and `ask` come only with `scope: "all"`. Without it, you
  read, wait on, send to and answer only sessions you spawned and theirs; with it, any session
  but your own, and your text reaches it opened `[From session <your id> via Luna]`. You stop,
  interrupt and delete only your own.
- `scope: "all"` is a person's chat's only. A helper (a session with a parent) reaches only
  itself and the sessions under it, in every tool: what is for anyone else goes in its reply.
  Nor does it notify the person or set the PC's power.
- `luna_session_stop` ends a helper's turn (no autoDelete after; a send goes on); between turns
  it ends what its turns left running. What outlives a stopped turn (on Windows a Git Bash
  command's processes, a `run_in_background` job's) shows in `leftRunning`, for a second stop.
- `luna_session_delete` takes `id` or `ids`; one deleted already answers `already: "deleted"`.
  `dropWorktree` removes its worktree and `worktree-*` branch too (one it named stays; ignored
  files go) unless another session is in it, and is refused if that loses uncommitted changes or
  unmerged commits (`force: true` overrides); what stays comes back as `worktreeLeft`.
- A deleted parent's helper passes to its parent; with none, only the user can stop or delete
  it, and it goes once idle for 30 min.
- A line (a person's chat and every session under it) has a number of places the user sets
  (8 unless changed); an autoDelete helper whose answer reached you takes none. `places` in a
  spawn's result and in `luna_session_list`: `inUse` of `of` on your line, `underYou` of them. A
  refusal names the ones you can delete; below the top, the top chat is told, as only it can
  free the rest. A helper given `tools: true` gets these tools too; its own helpers get none.
- `LUNA_SESSION_ID` holds a session's id, `LUNA_PARENT_ID` a helper's parent's. A helper is told it
  is one, not the chat of the person its instructions name (operator, user): Claude Code in its
  system prompt and after its first prompt, Codex in Luna's note on its prompts.

## Accounts and models
- An account is a signed-in login of one provider: an Anthropic account runs Claude Code
  only, an OpenAI account runs Codex only. `provider` and `account` in `luna_session_spawn` must
  match; an `account` alone brings its own provider. `luna_account_list` names those the user
  lets agents use, with their usage (% per window, when it resets) for the windows the
  provider reports: a window missing there was not sent, it is not at 0%.
- `luna_account_get` gives one: `defaults` (what a helper gets there unless the spawn says
  otherwise, from the account's and the project's settings, in `folder`, default yours), `usage`
  and `models` (Claude Code's aliases, a full id works too; Codex's catalog with each model's
  efforts).
- `account` as a list, of one provider: the first signed in with every usage window under 95%
  or unknown, else the most room. `accountChoice` says which and why.
- A usage window near 100% means a helper there may stop on its limit. When an account your live
  helpers run on reaches 90%, then 95%, of a window, a notice says so, once each until it resets.
- A model the organization does not allow is swapped by the CLI: `resolvedModel` says what ran.

## Spawning
- A safe spawn: a prompt that stands alone (the helper sees nothing of your conversation), the
  least mode that does the job (`readOnly`, `plan`, a read-only sandbox), a worktree for edits
  beside other work; then end your turn and let the notice come.
- A helper's mode (exact spelling) may not exceed yours: plan / read-only < default, dontAsk /
  read-only asking < acceptEdits < workspace-write < auto, auto-review < bypassPermissions /
  danger-full-access; network apart. Allow rules and config are not weighed. Higher: refused
  unless the user allows it. No helper works in or above Luna's own folder.
- No mode given (no `permissionMode`; for Codex neither `sandbox` nor `approval`): yours on the
  same CLI, else the highest of its modes on or below your rung (a Codex `approval` that asks
  a person becomes `never`), or the account's default if the user set so. `settings.modeFrom`
  says which: `caller`, `callerNearest`, `account`.
- Codex's `approval`: `never` (the default), or `auto-review`, a reviewer model answering what
  Codex would ask (workspace-write only). A helper has nobody to ask, so policies that ask a
  person are refused; the sandbox is what holds it. Running the project's hooks past Codex's trust
  prompt is the user's setting per folder: without it a Codex helper skips untrusted hooks. It
  trusts the folder it is sent to for its run, so the project's own `.codex/` config loads.
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
- `worktree: true` gives the helper a checkout of its own: its files and branch are apart from
  the folder's, but the repository's objects, refs and config are shared, so a branch it pushes
  or deletes is gone for all. The result names the worktree and branch, or `worktreePending` until
  the CLI has made it; spawns making worktrees in one repository wait their turn. `sameFolder`
  lists live sessions working in the same directory (from a helper, only those under it).
- A helper has Luna's MCP server alone: none of its account's servers or connectors (claude.ai's
  Gmail, Slack and the like; Codex's apps and `config.toml` servers), and a Claude Code one no
  auto-memory. `connectors: true` keeps them, for a task that needs them. A Codex helper still has
  servers from the project's `.codex/` and from plugins, which the result lists in `mcpLeftOn`; one
  whose account has a server Codex cannot turn off for a run (a name with a dot) is not started.
- `requestKey` (spawn, send, delete): a retry with the same key and arguments within an hour gets
  the first call's result, not a second helper or message; one that failed is made anew. A key
  stands for one call: other arguments under it are refused. Keys are kept in the core's memory:
  a core restart forgets them. A key is at most 128 characters; Luna keeps 64 of yours, the
  oldest finished making way. A result too large to keep comes back as what it made (`id`).
- `budgetTokens`: past that many `context` tokens (its own, not its subagents' or helpers') its
  parent hears, once; nothing stops. `settings` shows `budgetBreach`, `budgetTold`.

## Getting the answer
- After a spawn or a send you can end your turn. Once the session answers, asks something,
  stops on a prompt or exits, Luna types one line into your chat starting with
  `[Luna notice, not typed by the user]`. A short answer (up to 300 characters) is quoted whole.
  A longer one is in a file the notice names, its report first: the message its CLI marked as
  the turn's final answer, else the longest of its last turn (the notice then says it was picked
  by length); then its last message, then the rest in order. The notice quotes the report's
  first line (`It begins: "…"`), not of a turn that stopped on an error, which it says.
- A notice comes when your chat is between turns and nobody types in it. A subagent of yours in
  the background does not hold it back; another helper of yours ending within 20 s may, so that
  near-simultaneous ends come as one notice. A question or a prompt is never held.
- "NOT done yet": a job it started still runs (named), a subagent its CLI started works on, or
  its own helpers have yet to answer it. Said once. The next notice comes when that is over, or
  when it ends a turn on your words or on its own helper's last answer. A turn it ran only on a
  Luna notice brings none, unless it wrote a message of 400+ characters to you, its parent. All
  of it idle 30 min: one notice says so (stuck, or waiting).
- Past 12 turn-end notices about one session in 15 minutes Luna stops telling you of its turns
  and jobs (the last says so; its prompts and questions still come) until a person types into
  either chat; waits and reads still work.
- `luna_session_wait` gets the answer within your turn. `outcome`: `turn_done`, `waiting`,
  `exit`, `job_done` or `timeout`. `reply` is the report alone by default, with `rest` (how many
  more messages) and `file` (all of them, report first); `reply: "all"` puts them all in, `"none"`
  none. `failed` names the error a turn stopped on (API error, usage limit, crash, stop);
  `transcriptMissing`: no transcript, so no reply says nothing. `screen: true` adds the
  terminal. A helper does not exit between turns: `until: "exit"` returns once it is deleted.
- With `ids` (16 at most) it returns `outcome` (`finished` or `timeout`), `finished` and `results`, one per id
  as a single wait gives it, with `id`. Under `mode: "any"` the others come back `pending` or
  `timeout`, still owed. A wait running when the answer comes hands it over instead of the
  notice; one canceled hands nothing over, and the notice comes.
- `luna_report_get` gives the report alone of what it said since your question (or `since` a
  cursor), with `rest` and `file`, and starts its autoDelete count over. With `ack: true`, a
  report of all of an owed answer, its turn over, collects it: no notice follows (`acked`).
- `luna_message_list` gives messages from its transcript: `since` a cursor, `last` the newest N
  (default 1; with `since`, all after it). Past 60,000 characters, `rest` counts what was left
  out and `cursor` goes on from there. `toolCalls: true` marks tool calls (Claude Code);
  `screen: true` gives a helper's last log lines. A whole read of all it said since you asked,
  after it finished, collects the answer too.
- Asked again before an answer reached you, the next reply, read or notice covers all since the
  first question.
- A turn that has written nothing for 30 min is told of once, and not before 5 min past the
  timeout of a command it waits on: hung, or long. `screen: true` on a read tells which.
- Claude Code hands a background wait's result over HTML-escaped (`&lt;`, `&gt;`): take code
  from `luna_message_list`, not from that.

## Answering and steering
- A session in state `question` or `waiting` has an `ask`: the questions with their options, or
  the tool up for permission (`luna_session_get`; its row has the `askId`). In
  `luna_session_answer` an option's number counts from 1. Pass `askId`, so your answer cannot
  land on the next one; a stale or unknown id is refused, and so is no id with several held. It
  returns `state`, `ask` and, for a user's chat, the `screen` after it.
- `allow`: your helpers only, true only for what you run unasked (never plan/default). Others'
  (`scope: "all"`): a question its model asked, nothing else. No control chars. Text or a prompt
  starting with `/` or `!` (a CLI command, a shell as you) goes only at full access into your
  own helpers.
- A Claude Code helper's questions and permission prompts come to you this way, one at a time.
  A Codex helper's are in its reply.
- `luna_session_send` to a working session is `queued`: a Claude Code helper takes it at its next
  tool boundary (`joined`; a long tool or subagent delays it); a Codex helper the next turn; a
  user's chat the running turn or the next. Wait with the `turnsEnded` returned as `afterTurn`.
  `interrupt: true` (your own only) makes the text the next turn, behind any queued (`queued` if
  that turn would not stop). Where the user is typing, or left a draft Luna cannot put aside,
  the result says `held: true`: Luna types it in once they are done. Do not send again.
- `autoDelete` (on by default): Luna deletes the helper 30 minutes after its answer reached you,
  not while a job or helper of its runs (processes idle 2 h do not count: they go with it, and
  you hear so); a read, a report or a send starts that over (`deletesInMin` counts down). The
  worktree stays. One whose turn you stopped is never deleted.
- A deleted session's transcript stays in its account's folder (`luna_run_list` names it). If a
  helper printed what it should not have, tell the user that path.

## Restart
- A core restart stops a helper's turn and what its turns left running (off Windows only if it
  died). You stay owed: once your chat is back (given up after 10 minutes) a notice says it did
  NOT finish; a send has it carry on. What you had queued never reached it: the notice counts it
  and its file ends on it (`undelivered` in reads and waits).
- A helper stopped before it had a conversation "never got started": the file holds its prompt,
  then that text; a send starts it afresh. A worktree it began is reused only if Luna named it and
  it is untouched; else `worktreeLeft`.
- `resumeOnRestart` has it go on by itself (afresh on its prompt if it never got started); one
  notice says so, the usual one follows. Else the plain notice, also where Luna cannot tell the
  old turn ended, or where you wrote to it first.
- The user's chats you wrote to come back when the window starts them, with text held for them;
  the notice comes then, or after a minute saying they have not.
- A spawn a restart cut off errors. Wait for the notice before spawning again: no helper came of
  it (a file has its prompt; any `worktreeLeft`), or with `resumeOnRestart` it started afresh.

## Working with helpers
- With tools, and in every helper, a shell command following a file (`tail -f`, `Get-Content -Wait`)
  with no `timeout N` in front is refused (Codex: where its hooks run unasked).
- A helper acts as you. Messages, posts, emails, pushes or writes elsewhere in the user's name
  go out only when the user asked for them, by you or by a helper you brief.
- Relaying a report, keep its caveats ("possibly a false flag", "not tested live") with the
  claim. Ask reviewers and implementers to say which findings they think wrong, and why.
- Before ending a turn on "a notice will come", for anything long or overnight: a wait with a
  timeout, or a look at `luna_session_list` when the user is back. "NOT done yet" comes once.
- `luna_run_list`: past spawns, newest first, deleted helpers too, yours and your helpers'
  (`scope: "all"`: everyone's): `id`, `at`, `ended`, `how`, `name`, `account`, the start of the
  prompt and of the last reply, `transcript`. `q` searches names and prompts; `limit` (10).
- `luna_notification_send`: a desktop notification for a result or blocker the user must see now,
  not progress; one per 10 s. A turn ending on a question notifies anyway.
- Before anything that stops Luna's core or the PC: `luna_session_list` for helpers at work; they
  lose their turns. To turn the PC off use `luna_power_set`, not a `shutdown` command: it fires
  once Luna is quiet for `quietSec`, CLIs closed first; `cancel` disarms. `drain`: till then
  agents' spawns and sends are refused and answers take only a pending ask. After `restart` Luna
  starts at the next sign-in; that takes a person.
- `luna_guide_get` with no topic gives the topics; each tool's name is one.
