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
- **Auto-handoff at N% of its context**, off, only with Luna tools: see
  [Auto-handoff](#auto-handoff).

**Ctrl+Shift+N** opens the dialog instead when the folder is not trusted in the
account, since whether to trust it is yours to say.

**Claude Code:** model (a drop-down), effort
(`low/medium/high/xhigh/max/ultracode`) and permission mode. Isolation runs
`claude --worktree` in `<folder>/.claude/worktrees/<name>`, on a
`worktree-<name>` branch. Before Claude Code makes one, Luna fetches origin's
default branch (10 s at most, never asking for a password; off in Settings);
Claude Code branches off it and refetches only once a day itself.

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

Buttons first (recolor, rename, what Luna applied, delete, peek, take off the
board), then the name, its folder and, if the chat has background helpers, a
pill that sums them up (see [Helpers](agents.md#helpers)). The chat's settings
sit on the right: account, model and effort, permission mode (Claude Code) or
approval and sandbox (Codex; `bypass` and `auto-review` get chips of their
own), the worktree flag, context used as a meter and a number, and the session
id's first 8 characters (a click copies them; the full id is in the row's
tooltip).

A narrow pane gives up, in this order: the secondary chips and the session id,
the other chips and the meter's bar, the words of the helper pill (it keeps
its numbers), then the folder. The name and the pill's counts stay. Below
about 420px the recolor, rename, what Luna applied and delete buttons fold
into a **…** menu; peek, take off the board and "Not waiting" stay on the
bar.

Every new chat is dealt one of ten colors, worn by its title bar and as a stripe
on its sidebar row.

## What Luna applied

The ⓘ button on a chat's title bar lists everything Luna added to its CLI at
its latest start (a helper's latest turn), read-only, grouped by why it is
there:

- **You chose**: the model, effort and modes, the worktree, Luna tools, the wait
  guard, auto-handoff, hooks unasked, the library's skills and instructions, an agent's
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
**Wait guard for this chat** box (from the CLI's next start) and the chat's
**Auto-handoff** box ([below](#auto-handoff)). If Luna could not
note a write into the account, or its note of them will not read, the panel
says **Accounting incomplete** and why. A folder whose memory Luna could not
leave out without risking this project's own instructions is listed under
**Works around the CLI on this machine**.

Chats from before the guard was a choice, which had it because of Luna tools,
are named once in a notice across the board until you dismiss it. Below those comes what Luna wrote
into the account outside a start: the folder's trust when you ticked it, and
the library's copies. See [ADR-0016](../adr/0016-luna-is-a-runtime.md).

## Auto-handoff

A chat with Luna tools can go on in a fresh conversation of its own when its
context fills, written up by itself, its helpers carrying on. Tick
**Auto-handoff at N% of its context** in the new-chat dialog (70% unless you
change it, from 10 to 90), or later in the chat's **What Luna applied** panel.
A chat without Luna tools cannot have it: their server is what offers it.

- Once a turn ends past the threshold, Luna types one notice into the chat
  asking for a handoff. The chat calls `luna_session_handoff` without arguments
  and gets the steps and the instructions for the file: Luna's own, or yours
  from the library ([System skills](library.md#system-skills)). A firmer notice
  follows near where the CLI would compact the conversation by itself, carrying
  the file's headings itself (goal, next action, the user's words, state,
  pending, decisions and dead ends) for a chat short of context. A conversation
  a handoff began is asked only once it has grown 5 points past where it began.
  Nothing switches unless the chat asks.
- If the CLI compacts the conversation before a handoff, or the chat goes on
  with a model of a larger context window, it is asked again once it is past
  the threshold again. The chip on the title bar then reads
  **handoff N% · asked**, ringed, its tooltip saying at what share of the
  context it was asked and how often the CLI compacted it since (a larger
  window is no compaction); it goes once a handoff is under way or through.
- The chat writes the file in a folder of its own that Luna keeps, private,
  in its data folder (`handoffs/<chat>/drafts`); a Codex chat is given it as
  a folder its sandbox writes into. Where Luna could not make that folder
  private as the chat started, it gives and names none, and the chat writes
  the file outside Luna's folder, in its own workspace; the **Auto-handoff**
  line of What Luna applied says why. Under Codex's read-only sandbox, or in
  Claude Code's plan mode, it can write no file at all, and the tool's answer
  tells it so: you write the file, or give the chat a sandbox that writes.
- The chat calls `luna_session_handoff` with the file. Luna copies
  it into its data folder (written to Luna's own instructions, it names the
  headings the file lacks, and goes on) and, once the chat is at rest, types
  `/clear` (Claude Code) or `/new` (Codex) and then a prompt opening
  `[Luna handoff N` that points the new conversation at the copy and states what
  Luna knows itself, as of that moment: the chat's helpers with their model and
  effort, state, task, branches and worktrees and transcripts, the answers they
  owe, text not yet delivered, accounts and their use, the chat's settings. The
  prompt says that approvals the handoff quotes hold as far as your quoted
  words go and no further. Its helpers, their answers and notices carry on
  untouched; the chat keeps its row, name and id.
- Your draft in the chat holds the switch until you send or clear it; after a
  minute the desktop says so. Notices and messages for the chat wait meanwhile
  and go into the new conversation.
- If the switch fails before the reset (no new conversation, the CLI's own jobs
  still running, the chat never at rest), the conversation goes on as it was and
  a notice says why; you get a desktop notification too. If it fails after, the
  new conversation is told what it is and where to start.
- A divider over the terminal says **Handoff N · time**, with **open handoff**
  (Luna's copy of the file) and **previous conversation** (the one it left,
  read-only); from there **‹ before handoff N-1** steps further back. A chip on
  the title bar shows the threshold.
- Turned on while the chat runs, it takes effect at a quick resume of the chat
  on the same conversation once it is idle: a CLI lists Luna's tools as it
  starts. Turned off, the tool refuses from then on, and a handoff only
  scheduled is dropped.
- With it on, the chat's **What Luna applied** panel says whose instructions it
  would be given now: **Luna's** (with Luna's version and the text's hash),
  **yours** (the library's file and its hash), or **yours is broken** (why, and
  that Luna's is used). It warns when the chat's folder or account has a skill
  named `session-handoff` of its own, which the CLI may show the model and Luna
  never uses for the handoff.
- Only this chat is told of any of it: helpers, chats without the tick and
  chats without Luna tools are not offered the tool, find nothing of it in
  Luna's guide or in this chat's settings as they read them, and get nothing of
  it in their account or command line. A session that reads this chat's
  conversation (Luna tools let any read any other's) sees the notices in it.

See [ADR-0020](../adr/0020-session-self-handoff.md) and
[ADR-0022](../adr/0022-content-luna-serves-at-its-own-trigger.md).

## Status

- **Working / waiting for you / resting**, read from the CLI's own session
  registry, so it is right whether or not the chat has a pane.
- A chat turning to waiting raises a desktop notification (the window may be in
  the tray), with the system’s sound unless Settings → General → Notifications says not
  to. A click brings the window up with that chat on the peek sheet.
- **Waiting by marker:** a turn that ends asking you something in plain words is
  waiting too, when its last message ends on a line holding the text set in
  Settings → General → Waiting for you (`AWAITING OPERATOR` to begin with). It stays
  waiting until you write to it or clear it with the check on its title bar or
  row; a power-off armed for "when done" does not wait for it.
- Context use is read from the session transcript against the model's real
  window (from the Models API), so it costs nothing and cannot drift.

## Titles

Titles are the CLI's own. Claude Code names a session from its opening exchange
(the title it puts on a terminal tab) and writes it to the transcript; a
`/rename` wins over it. Codex threads use their name from
`session_index.jsonl`. The opening prompt stands in until a title exists. Rename
with the pencil on the row or the title bar; a name you set is never overwritten.

## Input, clipboard and links

- Ctrl+V pastes as in any terminal. With a screenshot on the clipboard it
  presses the CLI's own image-paste key (Alt+V for Claude Code, Ctrl+V for
  Codex).
- Files pasted or dropped into a pane are copied into the app's media store and
  their absolute paths typed into the prompt.
- Ctrl+C over a selection copies it; with nothing selected it interrupts the CLI.
  Selecting alone copies nothing.
- A link opens in your browser, chosen in Settings → Links: a default, and one
  per account or per project folder. Shift+click lists every browser, can
  remember the pick for that chat, and copies the link. Order: the chat's pick,
  its account's, its folder's, the default. A browser is a command line with
  `%1` where the link goes, so a profile is a flag
  (`--profile-directory="Profile 1"` for Chrome, Edge and Brave, `-P name` for
  Firefox). On Windows installed browsers and their profiles are listed
  automatically; elsewhere it is the system's handler and your own.
