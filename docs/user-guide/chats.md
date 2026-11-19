# Chats

![The new-chat dialog](../screenshots/new-chat.png)

## Creating a chat

**New chat** (Ctrl+N) asks for a folder and an account, plus the **Git worktree**,
**Luna tools** and **Wait guard** checkboxes. They start as Settings has them
(worktree on, tools and the guard off) or as kept for the folder: flipping one
is for that chat only, "back to …" undoes the flips, and "keep for this folder"
makes them the folder's own start; kept as Settings has them anyway, the folder
goes back to following Settings. Picking another folder starts the boxes the
way that one does, and a chat made with Ctrl+Shift+N takes that start. A
folder's start kept before the guard was part of it leaves the guard to
Settings. The folder list is the folders you launched chats in, adopted orphans
into or browsed to.

The account decides which CLI runs, and the rest of the dialog follows.

A folder a chat could not start in is said under the folder list, and Create
waits until another is picked: one that no longer exists, or, with **Git
worktree** ticked, one outside a git repository. Both CLIs make their worktree
with git, and Claude Code also through a `WorktreeCreate` hook: one named in
the account's or the project's settings lets the chat go. Where Claude Code
could get one from somewhere Luna does not read (a plugin you enabled, your
organization's or this machine's managed settings), the dialog only warns and
Create stays on: Claude Code decides, and exits at once if it has no hook. A
chat whose folder has gone since says so in its pane instead of starting.

These boxes add something to the CLI only when you ask for it:

- **Trust this folder in "<account>"** shows when the account has not trusted
  the folder yet, unticked. Ticked, Luna writes the same mark the CLI's own
  trust prompt writes (`hasTrustDialogAccepted` in `.claude.json`, or
  `trust_level = "trusted"` in `config.toml`), and the chat opens on its prompt.
  Unticked, nothing is written and the CLI asks in the chat itself (Codex then
  asks about its Windows sandbox too). Claude Code cannot ask when it makes a
  worktree, so such a chat needs the box ticked or the worktree off. A chat an
  agent starts never writes trust: a Codex helper is trusted for its own runs
  only.
- **Wait guard**, off unless Settings or the folder's start says on: a shell
  command that would wait forever (`tail -f`, `Get-Content -Wait` without a
  timeout) is refused before it runs, with the reason for the model. Helpers
  always have it. Codex runs it only with **Hooks unasked** on: without it the
  box shows off and cannot be ticked, whatever the start says, and the chat
  starts without the guard.

**Ctrl+Shift+N** opens the dialog instead when the folder is not trusted in the
account, since whether to trust it is yours to say.

**Claude Code:** model, effort and permission mode.

- Model: a drop-down of the models Claude Code offers this account, as its own
  `/model` does: each family's current model first (it runs by its alias,
  such as `opus`, so the chat follows the family to its next version), older
  versions under **More…** (each runs by its full id). Luna reads the list
  from the account's own Claude Code cache, or asks Claude Code for it in the
  background; until either has answered, and for an account that never
  signed in, it lists Fable, Opus, Sonnet and Haiku.
- Effort: the levels the chosen model takes (`low/medium/high/xhigh/max`
  today), plus `ultracode`. A model that takes no effort, such as Haiku 4.5,
  shows none. Claude Code keeps effort per model (what `/effort` sets in a
  chat is saved for that model), so each model opens on its own level, as
  Claude Code would pick it: the one the settings files keep for it, else
  the model's own default, and the line under it names the file and key.
  The account's top-level `effortLevel` counts only for older models, as in
  Claude Code; a project's or a managed one counts for every model. Picking a
  model moves to its level unless you picked one by hand; one that does not
  take the level shown moves it to that model's own default.
- Permission mode, passed as `--permission-mode`: **Ask** (`default`, confirm
  every action), **Edits** (`acceptEdits`), **Plan only** (`plan`), **Auto**
  (`auto`) and **Bypass** (`bypassPermissions`). In Auto Claude Code decides
  for itself, with its own review of risky actions. Where that review cannot
  run at all, it starts in its Manual mode, which asks first; where it cannot
  judge one action later on, it refuses that action ("Auto mode could not
  evaluate this action and is blocking it for safety"). Luna ranks the chat,
  and shows its chip, as Auto either way: a start in Manual lowers neither,
  and its helpers may still run in `auto` (Codex's `auto-review`), which is
  what you granted by picking Auto. A settings file's `defaultMode` of one of
  these opens the dialog on it; its `dontAsk`, or a mode Luna does not know,
  opens it on Ask; no `defaultMode` at all, on Bypass. Create waits until the
  files have answered.

Isolation runs `claude --worktree` in `<folder>/.claude/worktrees/<name>`, on
a `worktree-<name>` branch. Before Claude Code makes one, Luna fetches
origin's default branch (10 s at most, never asking for a password; off in
Settings); Claude Code branches off it and refetches only once a day itself.

**Codex:**

- Model: a drop-down from Codex's own catalog, in Codex's order: the list it
  caches for the account (`models_cache.json`), another account's when this one
  has never run, or `codex debug models` directly. The model a chat gets with
  nothing changed is marked "· default": the account's `config.toml` model, or
  the first Codex lists. It is always passed as `--model`.
- Reasoning effort as that model takes it (`low … ultra`, each level's
  description as a tooltip), opening on the model's default and following the
  model when another is picked, unless a settings file names a level or one was
  picked by hand.
- Approval policy (`on-request/never/auto-review`) and sandbox mode
  (`read-only/read-only-net/workspace-write/danger-full-access`). `never` +
  `danger-full-access` is passed as `--dangerously-bypass-approvals-and-sandbox`.
  `auto-review` is Codex's `--approve-for-me`: what Codex would ask you goes to
  a reviewer model, which approves or declines so the turn goes on, always in
  `workspace-write`.
- Isolation runs `codex --worktree`: Codex checks the current commit out into
  `<account>/worktrees/<4 hex>/<repo>` on a detached HEAD. A resumed chat goes
  back into that checkout.
- **Hooks unasked** (off in Settings, kept per folder like the boxes above): the
  project's hooks run without Codex's "Hooks need review" prompt
  (`--dangerously-bypass-hook-trust`). Codex asks again for every new worktree,
  account or edited hook; a background helper, which cannot ask, goes without
  them. A helper an agent starts takes the box as its folder would start it.

**Defaults come from the CLI's own files** for that account in that folder.
Claude Code: the account's `settings.json`, the project's
`.claude/settings.json` and `.claude/settings.local.json`, then the machine-wide
managed file. Codex: the account's `config.toml`, then the project's
`.codex/config.toml` (read only for a trusted folder). The line under each
control names the file it came from; changing one says so and offers the way
back. An override belongs to that chat and is never remembered as a default.

**Ctrl+Shift+N** skips the dialog: same folder and account as the chat on
screen, the rest as Settings has it. Either way the new chat lands in a pane you
can see: a free one, or the pane you last worked in when the board is full.

## The pane title bar

Buttons first (the **…** menu, delete, peek, take off the board), then the name, its folder and, if the chat has background helpers, a
pill that sums them up (see [Helpers](agents.md#helpers)). The chat's settings
sit on the right: account, model and effort, permission mode (Claude Code) or
approval and sandbox (Codex; `bypass` and `auto-review` get chips of their
own), the worktree flag, context used as a meter and a number, and the session
id's first 8 characters (a click copies them; the full id is in the row's
tooltip).

A narrow pane gives up, in this order: the secondary chips and the session id,
the other chips and the meter's bar, the words of the helper pill (it keeps
its numbers), then the folder. The name, the pill's counts and the buttons
stay.

The **…** menu holds what a chat needs now and then: **Rename** (or
double-click the name), **Chat color…** and **What Luna applied**.

Every new chat is dealt one of ten colors, worn by its title bar and as a stripe
on its sidebar row.

## What Luna applied

**What Luna applied** in a chat's **…** menu lists everything Luna added to its CLI at
its latest start (a helper's latest turn), read-only, grouped by why it is
there:

- **You chose**: the model, effort and modes, the worktree, Luna tools, the wait
  guard, hooks unasked, the library's skills and instructions, an agent's
  request.
- **Luna needs it to run the chat**: the account folder, the session's id,
  resume, the activity hooks, the CLI's own updates kept off, a helper's role,
  headless mode and trust for its run.
- **Protects Luna's own boundary**: the read deny on Luna's core folder; for a
  helper, none of the account's connectors or servers (unless the agent
  spawned it with them), with what a Codex helper keeps all the same (its
  project's and plugins' servers) listed beside it, and its auto-memory off.
- **Works around the CLI on this machine**: Codex's Windows sandbox when you
  set none, its PATH without the Store's aliases, git's OpenSSL backend.

Each item shows its value and the reason. Tokens and Luna's local addresses are
never shown, and files appear by their path. The panel also has the chat's own
**Wait guard for this chat** box (from the CLI's next start). If Luna could not
note a write into the account, or its note of them will not read, the panel
says **Accounting incomplete** and why. A folder whose memory Luna could not
leave out without risking this project's own instructions is listed under
**Works around the CLI on this machine**.

Chats from before the guard was a choice, which had it because of Luna tools,
are named once in a notice across the board until you dismiss it. Below those comes what Luna wrote
into the account outside a start: the folder's trust when you ticked it, and
the library's copies. See [ADR-0016](../adr/0016-luna-is-a-runtime.md).

## Status
- **Working / waiting for you / resting**, read from what the CLI itself says
  (Claude Code's hooks and session file, Codex's rollout), so it is right
  whether or not the chat has a pane. A chat Luna has heard nothing from yet
  reads working. A Codex turn held on an approval dialog reads working too:
  Codex records no such dialog.
- **Background:** the chat's turn is over, but work of its own goes on: a
  subagent, a background shell or a scheduled `/loop` its CLI names, a process
  a Codex turn left running, or a helper, or a helper's helper, that still owes
  it an answer. Its dot is a blue ring, where a working chat's is a filled
  green dot, and the status bar counts these chats and names them on hover. A
  chat that is waiting or working reads that instead. It reads background, and
  keeps the PC awake, for as long as that work is there, however long it does
  nothing ([Power](power.md#what-active-means)). When the work ends the dot
  goes back to resting, with no notification.
- A chat turning to waiting raises a desktop notification (the window may be in
  the tray), with the system’s sound unless Settings → General → Notifications says not
  to. A click brings the window up with that chat on the peek sheet.
- Context use is read from the session transcript against the model's real
  window (from the Models API), so it costs nothing and cannot drift.

## Titles

Titles are the CLI's own. Claude Code names a session from its opening exchange
(the title it puts on a terminal tab) and writes it to the transcript; a
`/rename` wins over it. Codex threads use their name from
`session_index.jsonl`. The opening prompt stands in until a title exists. Rename
with the pencil on the row, **Rename** in the pane's **…** menu or a
double-click on its name; a name you set is never overwritten.

## Input, clipboard and links

- Ctrl+V pastes as in any terminal. With a screenshot on the clipboard it
  presses the CLI's own image-paste key (Alt+V for Claude Code, Ctrl+V for
  Codex).
- Files pasted or dropped into a pane are copied into the app's media store and
  their absolute paths typed into the prompt.
- Ctrl+C over a selection copies it; with nothing selected it interrupts the CLI.
  Selecting alone copies nothing.
- Alt with an arrow key sends what Ctrl with it sends: Alt+Left and Alt+Right
  move a word, where the CLI's input takes such a move.
- A link opens in your system's default browser, or a mail link in your mail
  app, without a question. Only web (`http`, `https`) and mail (`mailto`) links
  open, and no mail link that asks to attach a file; any other is ignored.
