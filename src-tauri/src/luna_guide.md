# Luna: sessions, accounts and the luna_* tools

Luna is the desktop app this session runs in. It runs Claude Code and Codex sessions side by
side, each on one of the user's accounts, and through these tools any session that has them
can start other sessions and work with them.

## Sessions
- Every chat in Luna is a session: the user's own chats and the helpers sessions start. Each
  has an id, a name, a provider (`claude` = Claude Code, `codex` = Codex), an account, a
  project folder and a state: `working`, `waiting` (a permission prompt), `question`,
  `idle`, `starting` (its CLI has not drawn yet), `background` (a job of its still runs),
  `exited`.
- A helper is **background** by default: no row, no terminal; each turn is one run of the
  CLI's headless mode (`claude -p --output-format stream-json`, `codex exec --json`) on the
  helper's account, with the CLI Luna keeps for it, and the next turn resumes its
  conversation. The user sees it as a count
  on your chat's row and can open it as a chat between its turns. `visible: true` makes it a
  normal chat row instead, nested under yours, that the user can type into.
- `luna_list` covers what you spawned (and what they spawned); `luna_sessions` covers every
  session in Luna, the user's chats included, with its window group (I = 1). Window groups
  are how the user arranges chats; `group: "mine"` is yours.
- You kill and delete only sessions you spawned and theirs. Any other session you can read,
  wait on and send text to.
- At most eight helpers under one chat. A helper given `tools: true` gets these tools too,
  but its own helpers get none.

## Accounts and models
- An account is a signed-in login of one provider: an Anthropic account runs Claude Code
  only, an OpenAI account runs Codex only. `provider` and `account` in `luna_spawn` must
  match; take both from `luna_list`, which lists every account the user lets agents use
  (the user can switch accounts off for agents) under its exact name.
- Accounts of one provider differ: personal or team plans, different organizations, their
  own allowed models and their own quota. Each row in `luna_list` gives the provider, the
  name, the models it offers (for Codex with their efforts and default effort), the
  `defaults` a helper there gets in your folder (model, effort, permission mode or
  approval/sandbox, read from the account's and the project's settings) and `usage`: percent
  used per window and when each resets. A window near 100% means a helper there may stop
  on its limit.
- `model` takes one of the ids `luna_list` names for that account; Claude Code also takes a
  full model id. Leave a setting out to get the account's default.
- If the CLI swaps a model the organization does not allow, the spawn reports it in
  `warnings`.

## Spawning
- `luna_spawn` needs only `prompt`; everything else defaults to your own provider, account
  and folder.
- Claude Code: `permissionMode` = default | acceptEdits | plan | bypassPermissions;
  `effort` = low | medium | high | xhigh | max (ultracode too).
- Codex: `sandbox` = read-only | workspace-write | danger-full-access; `approval` =
  on-request | never (visible only: a background Codex runs with nobody to approve, the
  sandbox is what holds it); `effort` per model as `luna_list` gives it.
- `folder` runs it in another project. `worktree: true` gives it its own git worktree and
  branch, so it does not share files and git with the sessions in that folder; the result
  names the worktree and branch. `sameFolder` in the result lists live sessions working in
  the same directory.
- `name` is its name in Luna.
- The result: `id`, the resolved `settings`, `started` (false: the prompt was not taken
  up, `screen` shows why — an unknown slash command, a login screen), `starting` (the CLI
  has not drawn yet; the prompt is held, do not resend), `warnings`, `turnsEnded`.
- A prompt over 8,000 characters reaches a visible session as a file it is told to read.

## Getting the answer
- After a spawn or a `luna_send` you can end your turn. Once that session answers, asks
  something, stops on a prompt or exits, Luna types one line into your chat starting with
  `[Luna notice, not typed by the user]`. It quotes everything the session said since it was
  asked; past 3,000 characters it keeps the main report and names a file with the rest.
  The notice is delivered when your chat is between turns and the user is not typing in it.
- `luna_wait` blocks until a session ends a turn (and returns its reply), stops on a
  question or permission (`until: "waiting"`), or exits (`until: "exit"`); up to 30 min
  (`timeoutS`, default 600). Use it when you need the answer within the same turn. A
  `luna_wait` still running when the answer comes hands it over instead of the notice; one
  your CLI moves to the background still returns, with no notice besides.
- Turns are counted: results carry `turnsEnded`; pass it to `luna_wait` as `afterTurn` to
  wait for a turn after that one.
- `luna_read` reads the transcript: `last: N` messages (default 1), or `since: <cursor>`
  for everything after an earlier read; `tools: true` adds one-line tool calls, `screen:
  true` adds what the terminal shows (dialogs, errors) or, for a background helper, its
  last log lines.

## Answering and steering
- A session in state `question` or `waiting` has an `ask`: the questions with their
  options, or the tool up for permission. `luna_answer` takes `answers` (one per question:
  an option's number or label, or your own words), `allow: true/false` with an optional
  `note` for a permission, or `keys` (`{down}{enter}`, `{esc}`, `{tab}`, `{shift+tab}`,
  `{up}`, `{left}`, `{right}`, `{space}`, `{backspace}`) for any other dialog.
- A background Claude Code helper's questions and permission prompts come to you this way.
  A background Codex helper cannot ask; what it would have asked is in its reply.
- `luna_send` types text into a session as its next prompt. Sent while it works, the text
  is queued for after its turn (`queued`); `interrupt: true` stops the current turn first
  and makes the text the next one. It reads as the user's words, so name yourself to a
  session you did not spawn. It will not type into a chat where the user is typing or has
  left an unsent draft; it says so, and you try later.
- `luna_kill` stops a session's process (a background helper: its turn); the conversation
  stays for `luna_send` to continue. `luna_delete` removes it; `dropWorktree: true` also
  removes the worktree and branch Luna made for it.
- `autoDelete`: Luna deletes the helper 30 minutes after its answer reached you, unless you
  read or write to it in between (`deletesInMin` in `luna_list` counts down). On for
  background helpers unless `autoDelete: false`, off for visible ones unless `true`. A
  helper the user typed into, or one you killed, is never auto-deleted.
- `luna_notify` shows the user a desktop notification titled with your chat; a click opens
  it. One per 10 s. A turn that ends on a question already notifies.
- `luna_sessions` with `history: true` lists past spawns, deleted ones included: when, by
  whom, where, account, settings, the start of the prompt, how it ended, its transcript path
  and last message (`q` also searches prompts).
- Every helper costs its account's quota; several at once cost it in parallel.
