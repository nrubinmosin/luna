# Luna: sessions, accounts and the luna_* tools

Luna is the desktop app this session runs in: Claude Code and Codex sessions side by side, each
on one of the user's accounts. Each section below answers the topics `luna_guide_get` lists.

## Sessions
- Every chat in Luna is a session: the user's own chats and the helpers sessions start. Each
  has an id, a name, a provider (`claude` = Claude Code, `codex` = Codex), an account, a
  project folder and a `state` (also `stateBefore` in a send's result):
  - `working`: a turn is running.
  - `waiting`: stopped on a permission prompt; `question`: on a question that holds its turn.
    Both have an `askId` for `luna_session_answer`.
  - `background`: its turn is over, but a job or a subagent its CLI started, or a helper of
    its own, still runs: not done yet, though it takes a send as its next turn.
  - `idle`: between turns, nothing of its running.
  - `starting`: its CLI has not drawn yet.
  - `exited`: not running (deleted, or its CLI gone).
- Every helper runs in the background: no row, no terminal; each turn is one run of the CLI's
  headless mode (`claude -p`, `codex app-server`) on the helper's account, and the next turn resumes
  its conversation. The user sees it as a count on your chat's row, can watch it read-only, and
  can open it as a chat between its turns. Opening hands it to the user for good: no longer yours
  to send to, wait on, read, stop or delete; nothing more is owed by it; text still queued for it
  is not delivered; one notice tells you so, and where that text is.
- `luna_session_list` rows have `id`, `name`, `state`, `turnsEnded` and `askId` where available, plus `parent` if different from you.
  Yours by default; `scope: "all"` lists every session; `ids` selects sessions. Filter by
  state, provider, account, group (I = 1, or `"mine"`) or q (name, folder, account, id).
  Pass `nextCursor` as `cursor` for the next page (50 rows by default).
- `luna_session_get` adds provider, account, parent, task and the full ask. Request extra
  fields with `include` (a list): `settings` (effective flags and constraints), `paths`
  (folder, cwd if different, transcript), `diagnostics` (group, CLI/session IDs, turn and
  activity times in UTC, resolvedModel, context, leftRunning, deletesInMin), `places` (your line).
  `context.basis` is `autoCompact` or `model`; `contextToCompact` estimates tokens left after a delegate's first turn in this core.
- Another session's `task`, `askId` and `ask` come only with `scope: "all"`. Without it, you
  read, wait on, send to and answer only sessions you spawned and theirs; with it, you read and
  wait on any session but your own, and send to and answer the user's other chats, your text
  opened `[From session <your id> via Luna]`. Another's helper takes text and answers only
  from the sessions above it. If you try to write to it, Luna tells you whose it is.
  You stop, interrupt and delete only your own.
- `scope: "all"` is a person's chat's only. A helper (a session with a parent) reaches only
  itself and the sessions under it, in every tool: what is for anyone else goes in its reply.
  Nor does it notify the person or set the PC's power.
- `luna_session_stop` ends a helper's turn (no autoDelete after; a send goes on); between turns
  it ends what its turns left running. What outlives a stopped turn (on Windows a Git Bash
  command's processes, a `run_in_background` job's) shows in `leftRunning`, for a second stop.
- `luna_session_delete` takes `id` or `ids`; one deleted already answers `already: "deleted"`.
  Luna removes no worktree: one you made for a helper is yours to remove (`git worktree remove`
  once it is done and its work is merged or kept); `dropWorktree` is refused.
- A deleted parent's helper passes to its parent; with none, only the user can stop or delete
  it, and it goes once idle for 30 min.
- A line (a person's chat and every session under it) has a number of places the user sets
  (16 unless changed). Only a helper at work takes one (a turn running or waiting on a
  question), and a spawn under way; one between turns takes none, whatever it left running.
  Request `include: ["places"]` in `luna_session_get`: `inUse` (helpers at work and spawns
  under way on your line) of `of`, `underYou` of them, `waiting`: turns waiting for a place. A
  spawn past the limit is refused, naming yours at work: wait for a turn to end or stop one;
  below the top, the top chat is told, as only it can free the rest. Text for an idle helper
  is never refused for want of a place: it is queued (`waitsForPlace`), and its turn starts
  when one frees, in order. Luna's notices to a lead start its turn regardless. A helper given
  `tools: true` gets these tools too; its own helpers get none.
- `LUNA_SESSION_ID` holds a session's id, `LUNA_PARENT_ID` a helper's parent's. A helper is told its
  role in its instructions (Claude Code: memory; Codex: developer instructions), and a note after
  its task (Claude Code: the first; Codex: every one) says the task is from an agent, not a person.

## Accounts and models
- An account is a signed-in login of one provider: an Anthropic account runs Claude Code
  only, an OpenAI account runs Codex only. `provider` and `account` in `luna_session_spawn` must
  match; an `account` alone brings its own provider. `luna_account_list` names those the user
  lets agents use, with their usage (% per window, when it resets) for the windows the
  provider reports: a window missing there was not sent, it is not at 0%.
- `luna_account_get` gives one: `defaults` (what a helper gets there unless the spawn says
  otherwise, from the account's and the project's settings, in `folder`, default yours; Claude
  Code's effort is per model, `effortByModel`), `usage`
  and `models` (Claude Code's aliases, a full id works too; Codex's catalog with each model's
  efforts).
- `account` as a list, of one provider: the first signed in with every usage window under 95%
  or unknown, else the most room. `accountChoice` says which and why.
- A quota crossing (90%, 95%) goes to each person's top chat with live helpers
  on the account at that reading. Use `luna_account_list` for current usage.
- The person's top chat also gets breaker, folder-skills and places news.
  A delegate's own usage-limit stop is a failure reported to its parent.
- A model the organization does not allow is swapped by the CLI: `resolvedModel` (get diagnostics) says what ran.

## Spawning
- Helpers and leads use Luna's delegate auto-compaction setting.
- Use Luna helpers for work that must outlive your turn. Give a standalone prompt and the
  least mode needed. For edits, create a worktree, pass it as `folder` and remove it later.
  Git refs/config are shared. Folder defaults to yours; keep it outside, and never above, Luna's data folder.
- Wait on the returned `id` (first `afterTurn: 0`, also the default) or end your turn.
  `sameFolder` counts peers in reach sharing those files and the git index.
- Exact mode → CLI config → Luna Auto or Auto-review; unreadable Codex config → closed (`modeFrom`).
- Codex commits need `danger-full-access`. `readOnly` still allows Claude
  Bash writes and conflicts with write modes and auto-review. `agent` must name an existing Claude agent.
- Codex `read-only-net` allows reads and network; Windows schannel TLS fails.
- Codex sandboxing does not prevent data egress; Windows proxies leave some traffic open.
- Codex loads project config and runs trusted hooks only. `connectors: true` retains account
  integrations. Otherwise unreadable Codex config or managed integrations block the spawn.
  Provider memories are off.
- Constraints are instructions, not permissions; they append (4 KB/spawn, 16 KB total).
- Identical `requestKey` and args replay spawn/send/delete success for an hour in this core
  (128 characters, 64 keys). Failures retry. Changed args are refused.
- `budgetPct` and `budgetTokens` are ignored.

## Getting the answer
- End your turn for a Luna notice or wait. Short replies are quoted. `Report <path>:1–N` locates
  the report in the full file, with a first-line preview on success. Headers give the count
  and latest `#N (key)`. An extra #N labels an older preview; `[unmatched]` means no ordinary prompt matched.
  `Re request N` sections are newest first.
- `[idle; send to resume]` means only a send wakes it. `[send to resume]` follows interruption
  or error. Restart notices say if the helper resumed automatically.
- Fresh `DONE`, `BLOCKED`, `QUESTION`, `FAILED` or `PLAN` openings wake you after the turn ends.
  Asks and failures also wake. Each task gets one idle stall and a separate 30-minute quiet wake.
  A `PLAN` covers its turn. Other text stays readable. Luna wakes silent leads when delegates finish.
- After 12 turn/job notices per pair in 15 min, `[turn notices stopped]` says to use waits or reads.
- Wait takes up to 16 ids. Use `turnsEnded` for `afterTurn`. For a helper, exit means deletion.
  `outcome` is `turn_done`, `waiting`, `exit`, `job_done` (background ended), or `timeout`.
  With multiple ids, `finished` lists completed ids; `pending` leaves that answer owed.
  `failed` is a turn error; `stopped` is a Luna stop; `transcriptMissing` means reply unknown.
  `reportGuessed` marks a report picked without the CLI's final-answer marker.
  Cancellation leaves the answer owed. Read HTML-escaped Claude wait code via `luna_message_list`.
- A completed wait, `luna_report_get` with `ack: true`, or a full uncut read collects the
  finished answer. Follow cursors for cut messages. Requests before collection share the answer.

## Answering and steering
- A session in state `question` or `waiting` has an `ask`: the questions with their options, or
  the tool up for permission (`luna_session_get`; its row has the `askId`). A permission's
  `detail` is one field of the call (the command, path or URL), not all of it: a Write's content,
  a Task's prompt and other arguments are not in it. It is whole up to 8 KB, else ends
  `… [N more characters cut]` with `detailCut` counting them; a notice quotes only its start,
  marked when shortened, line breaks shown as `⏎`. `source` says whether the helper or a
  subagent of it asks. In
  `luna_session_answer` an option's number counts from 1. Pass `askId`, so your answer cannot
  land on the next one; a stale or unknown id is refused, and so is no id with several held. It
  returns `state`, `ask` and, for a user's chat, the `screen` after it.
- `allow`: your helpers only, any of their prompts, whatever either of you runs in. The user's
  other chats (`scope: "all"`): a question their model asked, nothing else. No control chars.
  Text or a prompt starting with `/` or `!` (a CLI command, a shell as you) goes only into your
  own helpers.
- A helper's questions and permission prompts come to you this way, one at a time: a Codex
  helper's approvals too.
- Text you send a Codex helper mid-turn steers that turn; a Claude Code helper takes it at its
  next tool call.
- `luna_session_send` to a working session is `queued`: a Claude Code helper takes it at its next
  tool boundary (`joined`; a long tool or subagent delays it); a Codex helper the running turn
  (steered), or the next if that turn ends first; a user's chat the running turn or the next. Wait with the `turnsEnded` returned as `afterTurn`.
  `interrupt: true` (your own only) makes the text the next turn, behind any queued (`queued` if
  that turn would not stop). Where the user is typing, or left a draft Luna cannot put aside,
  the result says `held: true`: Luna types it in once they are done. Do not send again.
- `expectReply: false` wakes only for a status, ask, or failure.
- `autoDelete` (on by default): Luna deletes the helper 30 minutes after its answer reached you,
  not while a job or helper of its runs, however long idle; a read, a report or a send starts
  that over (`deletesInMin` in get diagnostics). Unread quiet answers hold deletion up to 24 hours. A worktree you made for it stays. One whose turn you
  stopped is never deleted.
- A deleted session's transcript stays in its account's folder (`luna_run_list` names it). If a
  helper printed what it should not have, tell the user that path.

## Restart
- A core restart stops a helper's turn and what its turns left running (off Windows only if it
  died). You stay owed: once your chat is back (given up after 10 minutes) a notice says it did
  NOT finish; a send has it carry on. What you had queued never reached it: the notice counts it
  and its file ends on it (`undelivered` in reads and waits).
- A helper stopped before it had a conversation "never got started": the file holds its prompt,
  then that text; a send starts it afresh.
- `resumeOnRestart` has it go on by itself (afresh on its prompt if it never started).
  The next notice can mention the restart. If it cannot resume, the failure notice says so.
- The user's chats you wrote to come back when the window starts them, with text held for them;
  failure news comes then, or after a minute saying they have not.
- A spawn a restart cut off errors. Wait for the notice before spawning again: no helper came of
  it (a file has its prompt), or with `resumeOnRestart` it started afresh.

## Working with helpers
- A helper acts as you. Messages, posts, emails, pushes or writes elsewhere in the user's name
  go out only when the user asked for them, by you or by a helper you brief.
- Implementers and reviewers (yours, or a lead's) that must outlive a turn are Luna helpers
  (`luna_session_spawn`), never a CLI's native background subagents, which end with the caller's
  turn process. Luna does not create or remove worktrees: for isolation create one with `git
  worktree add`, pass it as `folder`, remove it when done.
- Relaying a report, keep its caveats ("possibly a false flag", "not tested live") with the
  claim. Ask reviewers and implementers to say which findings they think wrong, and why.
- Before ending a turn on "a notice will come", for anything long or overnight: a wait with a
  timeout, or check `luna_session_list` when the user is back.
- `luna_run_list`: past spawns, newest first, deleted helpers too, yours and your helpers'
  (`scope: "all"`: everyone's): `id`, `at`, `ended`, `how`, `name`, `account`, the start of the
  prompt and of the last reply, `transcript`. `q` searches names and prompts; `limit` (10).
- `luna_notification_send`: a desktop notification for a result or blocker the user must see now,
  not progress; one per 10 s. A turn ending on a question notifies anyway.
- Before anything that stops Luna's core or the PC: `luna_session_list` for helpers at work; they
  lose their turns. To turn the PC off use `luna_power_set`, not a `shutdown` command: it fires
  once no session has been active (a turn running or waiting, a shell, cron or subagent, a state
  not yet known) for the whole `quietSec`; `shutdown` and `restart` close the CLIs first,
  `hibernate` and `sleep` leave them running, and all but `shutdown` are Windows only;
  `cancel` disarms. `drain`: till then
  agents' spawns and sends are refused and answers take only a pending ask; as it fires all three
  are refused ("Luna is shutting down"): end your turn. It returns `busy`,
  the sessions active now, and of those `waitingForAPerson`. After `restart` Luna
  starts at the next sign-in; that takes a person.
- `luna_guide_get` with no topic gives the topics; each tool's name is one.
