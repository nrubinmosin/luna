# Luna: sessions, accounts and the luna_* tools

Luna is the desktop app this session runs in. It runs Claude Code and Codex sessions side by
side, each on one of the user's accounts, and through these tools any session that has them
can start other sessions and work with them.

## Sessions
- Every chat in Luna is a session: the user's own chats and the helpers sessions start. Each
  has an id, a name, a provider (`claude` = Claude Code, `codex` = Codex), an account, a
  project folder and a state: `working`, `waiting` (a permission prompt), `question`,
  `idle`, `starting` (its CLI has not drawn yet), `background` (a job or helper of its still
  runs), `exited`.
- A helper is **background** by default: no row, no terminal; each turn is one run of the
  CLI's headless mode (`claude -p --output-format stream-json`, `codex exec --json`) on the
  helper's account, with the CLI Luna keeps for it, and the next turn resumes its
  conversation. The user sees it as a count
  on your chat's row and can open it as a chat between its turns. `visible: true` makes it a
  normal chat row instead, nested under yours, that the user can type into.
- `luna_list` covers what you spawned (and what they spawned); `luna_sessions` covers every
  session in Luna, the user's chats included, with its window group (I = 1). Window groups
  are how the user arranges chats; `group: "mine"` is yours.
- Each `luna_list` row also says, with no need to read the session:
  - `turnStartedAt`: when the turn on now began; null between turns. `lastActivityAt`: the
    last thing Luna saw it do: a background helper's last event, a visible session's last
    transcript write. A long command or think shows nothing until it ends, so a quiet session
    is not yet a stuck one. Times are UTC ISO.
  - `resolvedModel`: the full model id the CLI used, after its first turn (Codex: what its
    rollout says the turn ran on, or the `<to>` of a `model rerouted` message).
  - `spend`: `session` and `lastTurn`, each `input` (no cache), `output`, `cacheRead`,
    `cacheWrite`, and `costUsd`: Claude Code's estimate, not a bill; Codex states none. A turn
    killed before it ended counts too, Claude Code's as tokens only; `lastTurn` is left out
    when it cannot be told. Visible sessions: Codex gives `session` only, Claude Code none,
    and a helper opened as a chat stops being counted.
- You kill and delete only sessions you spawned and theirs. Any other session you can read,
  wait on and send text to. A deleted parent's helper passes to its parent; with none, only
  the user can stop or delete it, and a background one goes once idle for 30 min.
- At most eight helpers under one chat. A helper given `tools: true` gets these tools too,
  but its own helpers get none.

## Accounts and models
- An account is a signed-in login of one provider: an Anthropic account runs Claude Code
  only, an OpenAI account runs Codex only. `provider` and `account` in `luna_spawn` must
  match; an `account` alone brings its own provider. Take them from `luna_list`, which lists
  every account the user lets agents use (the user can switch accounts off for agents)
  under its exact name.
- Accounts of one provider differ: personal or team plans, different organizations, their
  own allowed models and their own quota. Each row in `luna_list` gives the provider, the
  name, the models it offers (for Codex with their efforts and default effort), the
  `defaults` a helper there gets in your folder (model, effort, permission mode or
  approval/sandbox, read from the account's and the project's settings) and `usage`: percent
  used per window and when each resets. A window near 100% means a helper there may stop
  on its limit. When an account one of your live helpers runs on reaches 90%, then 95%, of a
  window, a notice says so, once each until it resets; no need to poll. The numbers are the
  ones Luna's window keeps, so with no window open none come.
- `model` takes one of the ids `luna_list` names for that account; Claude Code also takes a
  full model id. Leave a setting out to get the account's default.
- If the CLI swaps a model the organization does not allow, the spawn reports it in
  `warnings`.

## Spawning
- Codex's `approval`: auto-review has a reviewer model answer what Codex would ask
  (workspace-write only); on-request is for visible ones: a background Codex has nobody to
  ask, and runs with never unless told auto-review, the sandbox being what holds it. Whether
  it runs the project's hooks without Codex's trust prompt is the user's
  setting for that folder, not a spawn option; without it a background Codex runs untrusted
  hooks not at all, and a visible one stops on the prompt. A background Codex trusts the
  folder it is sent to for its run, so the project's own `.codex/` config loads; a visible
  one in a folder Codex does not trust yet has Luna trust it for that account.
- `folder` runs it in another project. `worktree: true` gives it its own git worktree and
  branch, so it does not share files and git with the sessions in that folder; the result
  names the worktree and branch, or `worktreePending` until the CLI has made it. `sameFolder`
  in the result lists live sessions working in the same directory.
- In the result, for a background helper `started` means its process is up. For a visible
  one, false means it was not seen on the prompt within 30 s, and `screen` shows why: an
  unknown slash command, a login screen, a dialog (then `ask` too).
- A prompt over 8,000 characters reaches a visible session as a file it is told to read.

## Getting the answer
- After a spawn or a `luna_send` you can end your turn. Once that session answers, asks
  something, stops on a prompt or exits, Luna types one line into your chat starting with
  `[Luna notice, not typed by the user]`. It says what happened and how many messages the
  session wrote since it was asked, and names a file holding them: the report (the longest
  message) first, then the last message, then the rest in order. The notice quotes only the
  report's first line, cut to about 150 characters (`It begins: "…"`), and not of a turn that
  stopped on an error, where it says so and why.
  The notice is delivered when your chat is between turns and the user is not typing in it.
- "NOT done yet": a job it started still runs, or its own helpers have yet to answer it. Said
  once; the next notice comes when that is over or a turn of its own ends, not one it only ran
  on a Luna notice, unless that was its own helper's last answer.
- Past 12 turn-end notices about one session in 15 minutes Luna stops telling you of its turns
  and jobs (the last says so; its prompts and questions still come) until a person types into
  either chat; `luna_wait` and `luna_read` still work.
- Use `luna_wait` when you need the answer within the same turn. `outcome` says which: `turn_done`,
  `waiting`, `exit`, `job_done` or `timeout`. On `turn_done` (and an `exit` after such a
  turn), `reply` holds all the session said since you asked, the report first, as the
  notice's file has it. `failed` names the error a turn stopped on (API error, usage limit,
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
- A `luna_wait` still running when the answer comes hands it over instead of the notice;
  one your CLI moves to the background still returns, with no notice besides. One you
  cancel or interrupt hands nothing over, and the notice comes as usual. A wait on several
  holds each answer the same way until it returns, and a cancel gives them all back.
- Without `afterTurn`, a wait is for the answer you are still owed, else for a turn past the
  last count you were shown.
- `luna_read` with `since` and `last` gives the last N after the cursor, and the cursor goes
  past all of them. `tools: true` is Claude Code only; `screen: true` gives a background
  helper's last log lines. A read with `last` that is cut keeps the newest, and its cursor is
  past the older ones too (read those from an earlier `since`). A single message past 60,000
  characters is cut short; the transcript `luna_sessions` names has it whole.
- A read that returns everything a session said since you asked, whole, after it finished,
  collects the answer: no notice follows. One with only the last message of several, or
  from a cursor taken after you asked, does not.
- Asked again before an answer reached you, you are owed both: the next reply, read or
  notice covers everything said since the first question.
- A restart of Luna's core stops a background helper's turn and what its turns left running. You
  stay owed: the notice comes once your chat is back (given up after 10 minutes), saying it did NOT
  finish; `luna_send` has it carry on. What you queued for it never reached it: the notice counts
  it and its file ends on it word for word (`undelivered` in `luna_read`, `luna_wait`). Stopped
  before its first turn got anywhere, it "never got started" and the file holds its prompt.
- `resumeOnRestart` has it go on by itself with that text (afresh on its prompt if it never got
  started) where Luna knows the old turn ended (a normal stop; on Windows any core death), else the
  plain notice. One notice says so; the usual one follows.
- Visible sessions and chats you wrote to come back when the window starts them, with text held for
  them; the notice comes then, or after a minute saying they have not.

## Answering and steering
- A session in state `question` or `waiting` has an `ask`: the questions with their
  options, or the tool up for permission. In `luna_answer` an option's number counts from 1.
  It returns `state`, `ask` and the `screen` after it. A background helper's `ask` has an `id`: pass it as `askId`, so your answer cannot land on the next one;
  a stale id is refused, and so is no id with several held.
- A visible Codex that stops on a list of its own — "Hooks need review" at startup, say —
  has it as a question too; its options are the only answers it takes. A spawn that did not
  start because of one returns it as `ask`.
- A background Claude Code helper's questions and permission prompts come to you this way, one
  at a time. A background Codex helper cannot ask; what it would have asked is in its reply.
- `luna_send` sent while a session works is `queued`: a visible session's CLI takes it into
  the running turn or the next, a background helper always the next; `luna_wait` with the
  `turnsEnded` returned waits for whichever answers it. `interrupt: true` makes the text the
  next turn, behind any queued (`queued` if that turn would not stop). The result says how
  it went, so there is no need to `luna_read` to check that the text arrived. Where the user
  is typing, or has left an unsent draft Luna cannot put aside
  (Claude Code's is put aside and comes back; Codex's cannot be), the result says
  `held: true`: Luna types the text in once they are done, and the answer comes as a
  notice, or to a `luna_wait`, which waits for the text to go in first. Do not send it
  again.
- `autoDelete`: Luna deletes the helper 30 minutes after its answer reached you, not while a
  job or helper of its runs; a read or a send in between starts the 30 minutes over
  (`deletesInMin` in `luna_list` counts down). The worktree stays. Off for visible ones unless
  `true`. A helper the user typed into, or one you killed, is never auto-deleted.
- `luna_sessions` with `history: true` also says when, by whom, where, on which account and
  settings each ran, the start of its prompt and how it ended.
- Every helper costs its account's quota; several at once cost it in parallel.
