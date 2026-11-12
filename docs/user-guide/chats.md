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
  [Handoff](#handoff).

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
  take the level shown moves it to that model's own default. Isolation runs
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

Buttons first (the **…** menu, delete, peek, take off the board, and "Not
waiting" while the marker has the chat waiting), then the name, its folder and, if the chat has background helpers, a
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
double-click the name), **Chat color…**, **What Luna applied** and **Move to
another account…**.

Every new chat is dealt one of ten colors, worn by its title bar and as a stripe
on its sidebar row.

## What Luna applied

**What Luna applied** in a chat's **…** menu lists everything Luna added to its CLI at
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
**Auto-handoff** box ([below](#handoff)). If Luna could not
note a write into the account, or its note of them will not read, the panel
says **Accounting incomplete** and why. A folder whose memory Luna could not
leave out without risking this project's own instructions is listed under
**Works around the CLI on this machine**.

Chats from before the guard was a choice, which had it because of Luna tools,
are named once in a notice across the board until you dismiss it. Below those comes what Luna wrote
into the account outside a start: the folder's trust when you ticked it, and
the library's copies. See [ADR-0016](../adr/0016-luna-is-a-runtime.md).

## Handoff

A chat with Luna tools can go on in a fresh conversation of its own, written
up by itself, its helpers carrying on. It does so only when it is asked:

- **Hand off now** in the pane's "…" menu asks at any time. The chat writes
  the handoff at the next point where its work can pause. The item is grayed,
  its tooltip saying why, where the chat could not answer: no Luna tools,
  Claude Code's plan mode or Codex's read-only sandbox, a handoff or a move
  of the chat under way, or an ask of yours already open. The core may still refuse
  (the CLI's own jobs running, its CLI not running); its reason then shows in
  a strip over the pane, × to dismiss. Luna types the request only once the
  chat stands at its prompt, with nothing in its composer. The chip on the
  title bar reads **handoff asked** (or **· asked** beside a threshold), and
  the menu item's tooltip says when the ask lapses, both as Luna's core holds
  them, so a reload of the window shows the same. Your ask lapses 10 minutes
  after Luna typed it if no handoff came; the chat is told, and the desktop
  says so. Until Luna has typed it, it asks nothing of the chat. One Luna
  could not type within 10 minutes of your ask (the chat never at its prompt,
  or its CLI not running) is withdrawn, with its own word to the chat and the
  desktop, and you may ask again. With the chat's CLI not running, the chip
  and the menu still follow what the core holds. Your ask takes the place of
  the threshold's; if it lapses or is withdrawn with no handoff, a chat still
  past its threshold is asked again at its next turn's end. A move of the
  chat to another account that you begin takes the place of your ask: it is
  withdrawn, and the pane and the desktop say so. A move cannot begin, or be
  sent, while a handoff of the chat is already under way.
- **Auto-handoff at N% of its context** asks by itself once the context passes
  a threshold. Tick it in the new-chat dialog (70% unless you change it, from
  10 to 90), or later in the chat's **What Luna applied** panel. A chat
  without Luna tools cannot have either: their server is what offers the
  handoff.

Unasked, the chat's `luna_session_handoff` tool refuses and names the pane's
menu, so words injected into the chat cannot hand it off on their own.

- Once a turn ends past the threshold, Luna types one notice into the chat
  asking for a handoff. A firmer notice follows near where the CLI would
  compact the conversation by itself, carrying the file's headings itself
  (goal, next action, the user's words, state, pending, decisions and dead
  ends, skills to load first) for a chat short of context. Ticked on a chat
  already past the threshold, Auto-handoff asks as soon as the chat is at
  rest, not at its next turn's end. A conversation a handoff began is asked
  only once it has grown 5 points past where it began, if Auto-handoff looks
  at it as its first turn ends; turned on after it has gone further, it asks
  as for any conversation. Nothing switches unless the chat asks.
- If the CLI compacts the conversation before a handoff, or the chat goes on
  with a model of a larger context window, the threshold's ask closes, and the
  chat is asked again once it is past the threshold again. The chip then reads
  **handoff N% · asked**, ringed, its tooltip saying at what share of the
  context it was asked and how often the CLI compacted it since (a larger
  window is no compaction); it goes once a handoff is under way or through.
- An ask holds only for the conversation it was typed into. Start a new one
  yourself (`/clear`, `/new`) and that one is asked nothing. A handoff the
  chat already called for and Luna has not yet switched is given up then:
  Luna clears nothing, keeps the file's copy, and tells the new conversation
  and the desktop. If Luna's core stopped in the middle of clearing it and the
  chat is then on a conversation Luna cannot tell its own `/clear` or `/new`
  opened, it is given up the same way: nothing typed there.
- Asked, the chat calls `luna_session_handoff` without arguments and gets the
  steps and the instructions for the file: Luna's own, or yours from the
  library ([System skills](library.md#system-skills)).
- The chat writes the file in a folder of its own that Luna keeps, private,
  in its data folder (`handoffs/<chat>/drafts`); a Codex chat is given it as
  a folder its sandbox writes into. Every chat with Luna tools gets that
  folder at its start, whether it hands off or not. Where Luna could not make
  that folder private as the chat started, it gives and names none, and the
  chat writes the file outside Luna's folder, in its own workspace; the
  `handoff-tool` line of What Luna applied says why. Under Codex's read-only
  sandbox, or in Claude Code's plan mode, it can write no file at all, and the
  tool's answer tells it so: you write the file, or give the chat a sandbox
  that writes.
- The chat calls `luna_session_handoff` with the file. Luna copies
  it into its data folder (written to Luna's own instructions, it names the
  headings the file lacks, and goes on) and, once the chat is at rest, types
  `/clear` (Claude Code) or `/new` (Codex) and then a prompt opening
  `[Luna handoff N` that points the new conversation at the copy and states what
  Luna knows itself, as of that moment: the chat's helpers with their model and
  effort, state, task, branches and worktrees and transcripts, the answers they
  owe, text not yet delivered, accounts and their use, the chat's settings, and
  the names of the skills the conversation before loaded (read off its
  transcript). The prompt tells the new conversation to load the skills the
  handoff lists under "Skills to load first" before its first reply, and says
  that approvals the handoff quotes hold as far as your quoted words go and no
  further. Its helpers, their answers and notices carry on
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
- A chat whose CLI started before this Luna listed the tool to it gets it at a
  quick resume of the chat on the same conversation once it is idle: on the
  first Hand off now, or when you tick Auto-handoff. A CLI lists Luna's tools
  as it starts.
- Turning Auto-handoff off closes only the threshold's ask, drops its notice
  if Luna has not typed it yet, and drops a handoff it only scheduled. Your
  own Hand off now stands. Turned on again, or set to another share, a chat
  past its threshold is asked as soon as it is at rest (after the quick
  resume, if its CLI needs one); a move of it, a handoff or your own ask
  under way puts that off until it ends.
- A Codex chat watching one of its subagents (`/subagents`) is still on its
  own conversation: its ask stays open.
- The chat's **What Luna applied** panel lists the tool as `handoff-tool`
  (on request) and the threshold apart, as `auto-handoff`. With Auto-handoff on, it says whose
  instructions the chat would be given now: **Luna's** (with Luna's version
  and the text's hash), **yours** (the library's file and its hash), or
  **yours is broken** (why, and that Luna's is used). It warns when the
  chat's folder or account has a skill named `session-handoff` of its own,
  which the CLI may show the model and Luna never uses for the handoff.
- Only your chats with Luna tools are told of any of it: helpers and sessions
  without Luna tools are not offered the tool, find nothing of it in Luna's
  guide or in a chat's settings as they read them, and get nothing of it in
  their account or command line. A session that reads a chat's conversation
  (Luna tools let any read any other's) sees the notices in it.

See [ADR-0020](../adr/0020-session-self-handoff.md),
[ADR-0022](../adr/0022-content-luna-serves-at-its-own-trigger.md) and
[ADR-0024](../adr/0024-handoff-on-request.md).

## Moving a chat to another account

When a chat's account runs out, the chat can go on on another account of the
same CLI: **Move to another account…** in the pane's "…" menu. A pane offers it
itself, in a strip under the title, when the account is at 90% of a usage window
or a usage limit stopped the chat's last turn; × dismisses the strip until Luna
starts again.

The conversation does not move: neither CLI resumes another account's. A new
chat opens there, named `<name> (moved)`, in the same folder, group and color,
with the same Luna tools, guard, auto-handoff and permissions, and the model and
effort the chat had where the other account lists them (else that account's
defaults). It goes on from a document, chosen under **The document**:

- **The chat writes it**, the default for a chat with Luna tools: Luna asks it
  through its handoff tool, once it is at its prompt, with the handoff's
  instructions in effect (yours from the library, if you have them) under the
  move's own rules. Nothing is cleared in it. Claude Code in its default mode
  asks you in its pane before it creates the file. Not offered, and why said
  beside it, for a chat without Luna tools, in plan mode or a read-only
  sandbox, not running, or stopped on a usage limit; a chat whose CLI started
  before it was listed the tool is to be resumed first.
- **Luna builds it from the chat's records**, with no model asked: how to go
  on, your note, the folder's state from git, how the chat stopped, the
  skills its conversation loaded, its task (its last handoff, or its first
  prompt), the plan it last proposed, the latest turns of the conversation,
  its helpers, the first steps. Tool output
  is never in it, nor the turns Luna's ask opened in the chat. Luna picks it
  for you only while a usage limit holds the chat, since the chat cannot
  write then: the dialog says when the chat stopped, and what is known of the
  window (which, how full, when it resets, how old the usage reading is). The
  limit holds until a usage reading taken after the stop shows the windows
  that count for the chat's model not full, or until their resets have
  passed; with no reading it holds, and so it does while a limit of the
  chat's model's own (Codex's Spark, say) has had no reading since the stop.
  The chat's model is the one its stopped turn ran on (a Codex chat switched
  to Spark is judged by Spark's limit; a Claude Code chat by the model that
  answered it last, a `/model` switch included, else the one it was started
  with). You can pick either one yourself.

**Ask the chat** opens the ask, in place of an auto-handoff ask the chat had
open (so does a document built from records), which asks again once the
move ends with the chat still running, if it is still past its threshold.
Your own Hand off now still open is withdrawn the same way, and you are told.
Only
the conversation Luna typed the ask into may answer it: start another one
(`/clear`, `/new`) and the ask fails at once, saying the chat went on in
another conversation, with **Ask again** offered.
The dialog then shrinks to a banner, so you
can answer the chat's CLI in its pane: waiting for its turn to end, writing
its document, or its CLI asking you something. **Use Luna's document now**
closes the ask and builds Luna's document instead; **Cancel the move** ends
it. A document the chat sends after its ask closed is refused and replaces
nothing.

If the ask fails (the chat did not come to rest within 10 minutes, wrote
nothing within 10 more, went on in another conversation, stopped on a limit,
or its CLI exited), the dialog
says why and offers **Build from records** (your instruction and choices
kept), **Ask again** where another ask may go through (not after a limit or
an exit), and **Cancel**. Luna never switches to its own document unseen.
Opened again, the dialog says why the last ask failed.

Either way you see everything that goes before it goes: who wrote the
document (**Written by the chat**, or **Built by Luna from the chat's
records** and why), the line Luna types into the new chat, and the document,
which you may edit. Luna has cut out the folders of accounts and its own,
taken out what looks like a key or a password, and says how many; long
random-looking strings are listed for you to look at. That is no guarantee:
something secret you pasted into the chat yourself is what to look for.
**Send** sends exactly the text shown.

The old chat can be kept or stopped. Kept, both chats work in one folder. A
Claude Code chat in a worktree is always stopped first, and the new chat goes
into the same worktree; while the new one runs there, the old one does not
start. A Codex chat in a worktree does not move: Codex keeps that worktree in
the old account's folder. Helpers stay with the old chat.

On an account no chat used yet, the CLI's first-run screens (a theme, security
notes, trusting the folder) show in the new chat's pane: answer them there;
Luna types its line only once the prompt shows. If something goes wrong, the
new chat stays with its document (its divider's **open document**), and a
notification says why.

## Status
- **Working / waiting for you / resting**, read from the CLI's own session
  registry, so it is right whether or not the chat has a pane.
- **Background:** the chat's turn is over, but work of its own goes on: a
  subagent, a job its turns started (a dev server, a watcher), or a helper, or
  a helper's helper, that still owes it an answer. Its dot is blue with a slow
  pulse, and the status bar counts these chats and names them on hover. A chat
  that is waiting or working reads that instead. A job that has done nothing
  for 2 hours on end stops counting, at the moment it stops keeping the PC
  awake. When the work ends the dot goes back to resting, with no notification.
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
- A link opens in your browser, chosen in Settings → Links: a default, and one
  per account or per project folder. Shift+click lists every browser, can
  remember the pick for that chat, and copies the link. Order: the chat's pick,
  its account's, its folder's, the default. A browser is a command line with
  `%1` where the link goes, so a profile is a flag
  (`--profile-directory="Profile 1"` for Chrome, Edge and Brave, `-P name` for
  Firefox). On Windows installed browsers and their profiles are listed
  automatically; elsewhere it is the system's handler and your own.
