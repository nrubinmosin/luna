# Luna

*[Русская версия](README.ru.md)*

A thin desktop shell (Rust + Tauri) around the Claude Code and Codex CLIs: several
sessions side by side in panes, project folders, and accounts with isolated configs —
an Anthropic subscription and a ChatGPT one in the same window. It carries its own copies
of the `claude` and `codex` binaries and keeps them current, so nothing here depends on a
global install.

The two CLIs are two separate layers. A Codex chat is described in Codex's own words
(model, reasoning effort, approval policy, sandbox mode), reads Codex's own `config.toml`,
and shows Codex's own rate-limit windows; nothing from the Claude side is mapped onto it.

![Four chats in one window](docs/screenshots/panes.png)

For how it is put together, see [ARCHITECTURE.md](ARCHITECTURE.md).

## The window

- A Windows XP shell (xp.css, Luna): the native window frame is replaced by our own
  title bar with minimize / maximize / close, and the UI runs on Segoe UI. The sidebar is
  Explorer's task pane, every control in every title bar is drawn from one set of
  glyphs, and every other button, the provider marks included, comes from one family of
  XP-style icons. Checkboxes, radios, fields and scrollbars are XP's, rounded off and
  themed. The whole interface scales from one variable, `--ui` in `src/app/theme.css`,
  and follows Windows' own text-size setting on top of that.
- The window's title bar carries the board's controls: the sidebar button, the four
  layouts, the window groups, Even out, Clear and the theme. A hidden sidebar takes
  no room at all: resting the pointer at the window's left edge slides it out over the
  board until the pointer leaves, and the title bar's button pins it back.
- Only the pane you are working in wears its full colour; the others go pale, the way XP
  drew inactive windows. A chat waiting for you flashes its title bar orange like an XP
  taskbar button, and over a pane you are not in a balloon says so.
- A grid of 1/2/3/4 panes with draggable splits (Ctrl+Shift+1..4 picks the layout).
  **One click** on a sidebar row shows that chat: with a single pane it simply becomes
  what that pane shows — dragging a row across an empty board to the one place it can go
  was ceremony — and with more panes a chat that is not on the board comes up on the peek
  sheet instead, so a click never rewrites an arrangement. Dragging is what seats a chat
  in a particular pane, and panes swap by dragging one title bar onto another.
- **Auto-tile** (on by default, Settings → Board): a new chat with no free pane grows the
  board by one pane, up to four, and lands in it — everything already on the board stays
  where it was. Taking a chat off the board or deleting it shrinks the board by one. Off,
  a new chat with nowhere to go comes up on the sheet and the board is left as arranged.
- **Peek** is the temporary view, and looks like one: the pane is held up on a sheet with
  the board still there behind it, dimmed. Ctrl+1..4 moves the keyboard into that pane,
  Alt+1..4 holds it up on the sheet and again puts it back; Ctrl+Shift+W
  goes to the next chat waiting for you, Ctrl+Shift+B pins or hides the sidebar;
  the numbered chips in the sheet's own bar move between panes, and a click anywhere
  outside the sheet, a second click on the same chat's row or
  on the chip of the pane that is up, the mouse's Back button, the Return button or Ctrl+0
  all go back. Holding Alt over a chat in the list is a glance: the chat comes up on the
  sheet and letting go of Alt puts back what was there (a click keeps it). The glance
  starts a beat after Alt goes down, so a chord with Alt (Alt+digit, Alt+V, Alt+Shift)
  never touches the sheet. Escape is
  deliberately left alone — it is how the CLI is interrupted.
- F1, or the **?** in the status bar, lists every shortcut and mouse gesture; the buttons
  themselves carry none. Nothing hides behind a double-click: rename, delete, even out
  the splits are all buttons. The chrome
  does not select as text, so a drag across the sidebar paints nothing blue.
- Luna claims its shortcuts in the capture phase, so they work while a terminal has focus;
  everything it does not claim travels on to the CLI untouched.
- Terminals outlive the pane that showed them: the last three put down wait off-screen with
  their listeners still on, so switching back is a refit rather than a fresh session
  handshake and a 2MB replay of the scrollback.
- Window groups — beside the layout selector on the title bar, one to start with and up
  to ten (I–X): **+** or Ctrl+Shift+T adds one (Ctrl+T is left to the CLI), **Remove**
  takes the group on screen away once no chat is left in it. Ctrl+Tab goes round them
  (Ctrl+Shift+Tab is left to the CLI). Each group remembers its
  own four boards (one per layout) and their splits; **Clear** empties the group's panes
  and leaves every chat alone, **Even out** puts the splits back to half.
- The status bar runs the width of the window: how many chats are working, how many are
  waiting (a click goes to the next one), and every account with every one of its limit
  windows — how full, and how long until it resets (a click opens the Accounts window) —
  then power, settings and help. When the bar runs out of room each account keeps its
  name and its fullest window. There is no clock — the taskbar has one.
- Light / dark / system themes, toned for reading all day: no pure black or white and no
  blue cast in any surface, surfaces stepped apart by lightness, the terminal the deepest
  of them, ANSI colours softened but at 4.5:1 or better. Light is XP's own warm beige
  under its blue title bars, on warm paper; dark is neutral graphite.

![The dark theme, two panes](docs/screenshots/dark.png)

## Chats

- New chat (Ctrl+N): folder, account, and the **Git worktree** and **Luna tools**
  checkboxes. They start as Settings has them (worktree on, tools off), or as kept for the
  folder: flipping one is just for that chat, and "keep for this folder" under them makes
  it the folder's own start. Isolation is each CLI's own `--worktree`. Before Claude Code
  makes one Luna fetches origin's default branch (10 s at most, never asking for a
  password; off in Settings): Claude Code branches the worktree off it and refetches it
  itself only once a day.
  A Codex account adds **Hooks unasked**, which starts and is kept the same way (off in
  Settings): the project's hooks run without Codex's "Hooks need review" prompt
  (`--dangerously-bypass-hook-trust`). Codex asks again for every new worktree, account or
  edited hook, and a background helper, which cannot ask, goes without them. A helper an
  agent starts takes the box as its folder would start it; the agent cannot choose it.
  The account decides which CLI the chat runs, and the rest of the dialog follows:
  - **Claude Code**: model (a drop-down), effort (`low/medium/high/xhigh/max/ultracode`)
    and permission mode. Isolation runs the session as `claude --worktree` in
    `<folder>/.claude/worktrees/<name>`, on a `worktree-<name>` branch.
  - **Codex**: model — a drop-down off Codex's own catalogue, in Codex's order: the list
    it caches for the account (`models_cache.json`), another account's when this one has
    never run, or `codex debug models` asked directly — never a box to type a slug into.
    The model a chat gets when nothing is changed is named and marked "· default": the
    account's `config.toml` model, or else the first one Codex lists, which is the one it
    would pick; it is always passed as `--model`. Reasoning effort as that model takes it
    (`low … ultra`, from the same list, each level's description as its tooltip), opening
    on the model's own default level and following the model when another is picked —
    unless a settings file names a level or one was picked by hand. Approval policy
    (`on-request/never/auto-review`) and sandbox mode (`read-only/read-only-net/workspace-write/danger-full-access`)
    are picked separately; `never` + `danger-full-access` is passed as
    `--dangerously-bypass-approvals-and-sandbox`, as Codex spells it. `auto-review` is
    Codex's `--approve-for-me`: what Codex would ask you goes to a reviewer model, which
    approves or declines so the turn goes on, always in `workspace-write`. Isolation runs
    `codex --worktree`: Codex checks the current commit out into
    `<account>/worktrees/<4 hex>/<repo>` on a detached HEAD, and Luna reads where from the
    session's rollout. A resumed chat goes back into that checkout.
- Those controls open on whatever the CLI's own settings say for that account in that
  folder. Claude Code: the account's `settings.json`, then the project's
  `.claude/settings.json` and `.claude/settings.local.json`, then the machine-wide managed
  file. Codex: the account's `config.toml`, then the project's `.codex/config.toml` (which
  Codex, and so Luna, only reads for a trusted folder). The line under each control names
  the file it came from, and changing one says so and offers the way back — an override
  belongs to that chat and is never remembered as the new default.
- Ctrl+Shift+N skips the dialog: same folder and account as the chat on screen, the rest
  as the settings have it, the two checkboxes as the folder starts them. Either way the new chat lands in a pane you can see — a free
  one, or the pane you were last working in when the board is full.
- A chat's pane title bar starts with the buttons to recolour, rename, delete, peek it or
  take it off the board — on the left, where the hand gets first — and then its name and,
  as separate chips, folder, account, model and effort, the permission mode (Claude Code)
  or approval and sandbox (Codex), the worktree flag, and how much of the context window
  is gone, as a meter and as a number. A narrow pane drops the chips one by one before
  the name and the buttons.
- Colours: every new chat is dealt one of ten presets, worn by its pane's title bar and as
  a stripe on its sidebar row, so a glance links the two without reading either name.
- Status per chat — working / waiting for you / resting — read from the CLI's own session
  registry, so it is right whether or not the chat has a pane. A chat turning to waiting
  raises a desktop notification, since the window may well be in the tray. It plays the
  system's notification sound unless Settings → Notifications says not to, and a click on
  it brings the window up with that chat on the peek sheet, the board left as it was.
  A turn that ends asking you something in plain words is waiting too: when the agent says so
  with `luna_waiting` (chats with Luna tools), or when its last message ends on a line holding
  the text set in Settings → Waiting for you (`AWAITING OPERATOR` to begin with). It stays
  waiting until you next write to it; a power-off armed for when all is done does not wait
  for it.
- Context use is read straight out of the session transcript against the model's real
  window (fetched from the Models API), so it costs nothing and cannot drift.
- Titles are the CLI's own: Claude Code names a session from its opening exchange — the
  same title it puts on a terminal tab — and writes it into the transcript, where Luna
  reads it for free; a `/rename` wins over it. Codex threads use their name from
  `session_index.jsonl`. The opening prompt only stands in until that title exists.
  Rename with the pencil on the row or the title bar; a name you set by hand is never
  overwritten.
- The session id's first 8 characters — the form issue claims and logs quote — sit on the
  pane's title bar (a click copies them), and the full id is in the row's tooltip.
- The number on a row is what Ctrl+&lt;digit&gt; reaches: the pane it is showing in, or its
  place in the list when there is only one pane. It is filled in while the chat is on the
  board.
- Ctrl+V pastes as in any terminal. With a screenshot on the clipboard it presses the
  CLI's own image-paste key (Alt+V for Claude Code, Ctrl+V for Codex), so the CLI
  attaches it the way it does anywhere.
  Files pasted or dropped into a pane are copied into the app's own media store and their
  absolute paths are typed into the prompt, which is the only thing a webview leaves usable.
- Ctrl+C over a selection in a pane copies it; with nothing selected it interrupts the CLI
  as ever. Selecting alone copies nothing.
- A link in a pane opens in your browser, not in a window of Luna's. Which one is set in
  Settings → Links: a default, and one per account or per project folder. Shift+click on a
  link lists every browser to open it in, can remember the pick for that chat, and copies the
  link. A chat's own pick goes first, then its account's, its folder's, the default. A browser
  is a command line with `%1` where the link goes, so a browser profile is a flag
  (`--profile-directory="Profile 1"` for Chrome, Edge and Brave, `-P name` for Firefox). On
  Windows the browsers installed and their profiles are listed by themselves, the system's
  handler with its flags among them; elsewhere it is the system's handler and your own.

![The new-chat dialog](docs/screenshots/new-chat.png)

## Accounts

- An account is a folder under one root, by default `Documents/luna-accounts`, split by
  vendor: `anthropic/<name>` for Claude Code, `openai/<name>` for Codex. Everything about
  accounts lives in the **Accounts** window, opened from the status bar: "Add account" asks
  which CLI and creates the folder, the bin deletes it after a confirmation, and Settings
  moves the root anywhere. A Claude Code session starts with
  `CLAUDE_CONFIG_DIR=<account folder>`, a Codex session with `CODEX_HOME=<account folder>`,
  so logins, settings and history are per account either way. A fresh Codex account gets a
  `config.toml` with `check_for_update_on_startup = false` — Luna updates that CLI itself.
- Live limits cost no tokens. Claude Code: 5h / week / every per-model weekly limit, each
  with its reset (in so long, and the exact date and time in the Accounts window), from
  the OAuth usage endpoint `/usage` itself uses, with the CLI's own cache answering most
  rounds. Codex: the rate-limit windows of the ChatGPT plan (typically 5 hours and a
  week, plus any per-model limits), from the same endpoint Codex's `/status` reads, with
  the token in the account's `auth.json`; when that is unreachable, the last
  `token_count` line of a rollout has the same numbers as of the last turn. Either way a
  429 backs off for longer each time rather than hammering through it, and the refresh
  button on the account's row asks right now.
- A Claude Code access token lasts about eight hours; an account nobody has opened since
  gets its token renewed by Luna the way the CLI does it — under the CLI's own lock, so
  a session running beside it picks up the new token instead of fighting over it.
- Signing in a fresh account is a button on its row: it runs `claude`, or `codex login`,
  in a modal, so there is no reason to leave the app.
- Luna writes the CLI's own trust bit for a folder when it has to. Claude Code cannot show
  its trust prompt under `--worktree`, which used to mean opening every new folder once
  without isolation first; Codex would follow its trust screen with a Windows sandbox
  setup question. Luna writes `hasTrustDialogAccepted` into `.claude.json`, or a
  `[projects.'<path>'] trust_level = "trusted"` entry into `config.toml`, before the
  session starts. Codex can also be told for one run (`-c projects=…`), which writes
  nothing: a background Codex helper, which cannot ask and would otherwise go without the
  project's `.codex/` config and hooks, is trusted that way wherever it is sent, and so is
  any Codex session in a project under a folder listed in Settings → "Codex trusts on
  this computer". Codex itself matches trust by a project's exact path, so a parent folder
  in its config would not do.
- **Recommended settings**, per account in the Accounts window — its button says how
  many of them the account's file has (`5/12`): what works well for the CLI inside Luna,
  each with what the file says now, what it would say and why — `tui: fullscreen`, no
  spinner tips or feedback prompts, no compacting while idle, no second Bypass confirmation, PowerShell for
  `!` commands on Windows only, no copy on select (Claude Code, which
  keeps that one in `.claude.json`); no update check, tooltips, desktop notifications,
  copy on select, analytics or feedback prompts, live web search (Codex); and as matters
  of taste attribution, Bypass by default. Nothing is written
  until asked. A tick is what the file will have: the keys already there open ticked,
  and unticking one takes it back out (only while it still says what was recommended);
  "Tick recommended" ticks the everyday set, and Apply says how many keys go in and out.
  The rest of the file, comments included, stays.
- An account can be renamed in place (the pencil beside its name): the folder is renamed
  and every chat on it, the agents' list and the settings parent follow — not while a
  chat or a background helper on it is running. Its settings file opens in an editor too (`settings.json` for
  Claude Code, `config.toml` for Codex; also from the recommended settings), with syntax
  colours and line numbers. It is checked as it is typed — the line that breaks it is
  marked — written only once it parses, and laid out on save the way the CLI writes it
  (JSON indented by two, TOML by taplo's rules), comments and key order kept; Ctrl+S
  saves.
- **Shared settings**: pick a parent account per CLI and its settings file is copied onto
  the others — now, with Sync now, or at every launch with the box ticked. The others get
  an exact copy, so a key taken out of the parent goes from them too and whatever was set
  on a child alone is replaced; logins and Codex's trusted folders stay each account's own.

## Skills and instructions

- Luna keeps a library of your own: instructions files, each with a name, and any number of
  skills, in a folder of its own (`library` in Luna's data folder) or one you choose — a
  repository of your dotfiles, say. The **Skills & instructions** window, from the Accounts
  window, makes, renames and edits them and says where each one goes. Instructions are
  told apart there by their first heading; each name has a copy button, to name a skill or
  instructions in a chat.
- Each skill, and each instructions file, goes into the accounts you pick for it: every
  account, every account of one CLI, or accounts by name — and an account that is renamed
  keeps what was picked for it.
- Instructions go into their accounts as `CLAUDE.md` in a Claude Code account's folder,
  `AGENTS.md` in a Codex one — the files each CLI reads at the start of every session, in
  every project. One file serves both CLIs, and an account gets one at most: the window shows
  instructions against accounts, a radio per account for the one it gets, or none. Where
  several take an account in, the one that names it wins over one for every account of its
  CLI, and that over one for every account — so `main` for everyone and `work` for the work
  account just works. Giving one instructions file every account, a CLI's or an account by
  name takes that from whichever had it. An account that already has its own file keeps it
  until you take it into the library or replace it; one that gets no instructions from the
  library can have its own taken in as new instructions, named after it and going into it.
  The single `instructions.md` of earlier versions becomes `instructions/main.md`, for the
  same accounts.
- A skill is a folder with a `SKILL.md` (front matter with its `name` and a `description`,
  then the instructions) and whatever files it needs beside it. In each account picked for
  it, it goes into the account's `skills/`, for every session in every project.
- A folder you add — a repository, say — is for what belongs to one project but not to
  everyone who works on it. A skill ticked for it goes to the sessions Luna starts there,
  its worktrees included (helpers, resumes and the CLIs' own subagents too), given to each
  at its start: nothing is written into the folder, so nothing shows up in `git status` and
  nothing is missing from a fresh worktree. Claude Code gets the skills through `--add-dir`
  of a folder in Luna's data and lists them by their own names; one whose name the
  repository or the account already has goes by `luna-<name>` there. Codex gets them as a
  plugin in the account, switched on for the session, and lists them as
  `<folder>:<name>`. Sessions started outside Luna do not see them.
- Each skill takes its accounts and its folders one of two ways. **or**, the default: the
  accounts picked get it everywhere, and every account's sessions get it in the folders it
  is ticked for. **and**: only the accounts picked, and only in those folders — a skill for
  one repository on every account but one, say.
- A folder can also keep a skill from its sessions — a repository with a skill of its own
  that does the same job, say. A faint tick is a skill the folder's sessions get anyway, from
  the accounts it is meant for or a folder this one lies in; a click turns it off there, a
  red cross, and the next back. Luna hides it from the session at its start
  (`skillOverrides` in Claude Code's `--settings`, `skills.config` in Codex's `-c`); the
  account's copy stays, so a CLI started outside Luna there still has it. Both CLIs hide a
  skill by name, so one of the same name the repository has goes too. Where one folder lies
  in another, the inner one decides — off in a parent folder, ticked again in one repository.
- "Add a folder…" lists the folders the new-chat dialog offers, and any other through the
  picker: one holding several repositories gives its skills to each.
- The library is the original and every place holds a copy. Luna remembers what it wrote
  where, so it touches only its own copies: anything else in an account or a folder stays,
  and a copy no longer wanted goes only while it is still exactly what Luna wrote. A copy
  changed or deleted where it lies — by hand, or by a model improving a skill in place — is
  never silently replaced: it shows up under "Needs a look", and as a chip on the status bar,
  with its differences from what Luna wrote. Take it into the library and it becomes every
  copy's; put the library's back; or leave that place out. Whichever side is replaced is kept
  in Luna's backups for 30 days.
- The library is laid out when Luna starts, before every session Luna starts (a chat, a
  resume, each turn of a helper), on every change made in the window and on Sync now: a file
  changed in the library by another program — a model improving a skill there, say —
  reaches the accounts by the next session. It is looked at again whenever the window comes
  back to the front, so a copy edited in another program is noticed then. A skill whose `SKILL.md` does not do (no front matter, a
  name other than its folder's) is laid out nowhere until it does, and its copies stay as they
  were; so do those of a library that will not read for a moment (a file held open mid-save).
  Paths are compared as the system compares them — on Windows regardless of case — so an
  account renamed in case only keeps its copies. A Codex account with an `AGENTS.override.md` reads that instead of `AGENTS.md`; the
  window says so.

## Sessions and cleanup

- Closing the window hides the app to the tray and leaves the sessions running. Quit from
  the tray menu ends all of Luna: every session is asked to quit as deleting a chat would,
  a background helper's turn is stopped, then the core and the window go. A core that has not gone 30 seconds later is killed
  along with everything it started.
- **Restart window** in the tray menu starts only the window over, for when the page
  goes wrong (terminals drawing garbage, say): the core and its sessions keep running,
  turns in progress included, and the new window picks them up.
- Sessions with no chat row left to reach them — lost to a half-written state restore, say
  — are listed at the top of the sidebar, with what they are
  working on, and can be adopted back or killed.
- **Worktrees** on the title bar lists the worktrees Claude Code and Codex left behind with
  no chat in them, by folder and by CLI, with a count on the button when there are any.
  Each is looked inside first: the clean ones start ticked, the ones holding uncommitted
  files or commits that deleting them would lose (on no branch that stays, nor a remote) say so. Delete them one by one or all ticked at
  once; nothing a live session or a background helper sits in is ever deleted.
- Deleting a chat takes its worktree, and the branch Claude Code made for it,
  along by default, unless another chat or helper still works in it; the checkbox can be unticked per chat, and where it
  starts is set in Settings.
- Both halves of the app log to `luna.log`, next to the exe, or under `%LOCALAPPDATA%\luna`
  when that folder is not writable: warnings and errors, and the events a problem is read
  back from — each session started, exited and deleted, each helper turn, each MCP call (who
  called which tool on which session, how it ended, how long it took), each notice, and the
  awake hold set and let go. Sessions are named by id and chat name, so a deleted one can
  still be told; what is said in a chat — prompts, sent text, answers — never goes in. Every
  line names the version that wrote it, and each launch opens with a `START` line naming the
  version and build.
- The sessions run in Luna's core, a second process from the same exe (`luna-core.exe`,
  started from a copy under the data folder on Windows so that an update can replace the
  exe), not in the window. A window that crashes, hangs or is killed takes nothing with it:
  the CLIs carry on, background work and all, and the next window reattaches to them. A
  window that falls behind on the sessions' output is let go by the core and goes straight
  back to it, its terminals redrawn from the scrollback. The
  hooks' listener, the agents' MCP server, activity and power run there too, so an agent's
  `luna_wait` keeps working with no window open. Settings → Versions shows the core and can
  restart it; it leaves by itself once it has had no window and no session (a background
  helper in a turn counts as one, and so does one whose turns left work running, for as
  long as that work keeps the PC awake) for ten minutes.
  If the core itself stops, the sessions in it end: a banner over the board says so, and
  Restart the core brings the chats back from their transcripts. It is never restarted
  behind your back. Background helpers outlive the core too: each is kept in
  `helpers.json` in the data folder (its settings, its conversation's id, whose it is —
  not what was said), and the next core takes them up, after an update, a restart or a
  reboot. A turn the core was in the middle of is stopped with it; the agent owed the
  answer is told so once its own chat is back, along with any messages queued behind that
  turn, kept word for word to send again, and `luna_send` has the helper carry on. A helper
  spawned with `resumeOnRestart: true` goes on by itself, those messages included, where Luna
  knows the old turn ended (a normal stop or any death of the core). One stopped
  before its first turn got anywhere is reported as never started, with its prompt, and with
  `resumeOnRestart` starts afresh on it. A visible helper spawned with it goes on too: Luna
  starts its CLI, in a pane or not, and types the same word in once it is back at its prompt,
  unless you typed into it or an agent wrote to it first. A visible helper without it that has
  no conversation to come back to is reported as never started in the same way, the prompt
  and that agent's messages Luna was holding in a file, to send again. Luna names a Claude Code
  helper's worktree itself, so a fresh start goes back into that one and no other, and only if
  nothing has touched it since Claude Code made it. Anything else, and whatever a Codex helper
  began (Codex takes no name, so nothing says whose a worktree is), stays where it is and the
  agent hears of it as `worktreeLeft`. What agents hold
  of the chats you see — who spawned them, a worktree they were handed, answers owed, messages Luna was holding
  for your draft — is kept in `chat-ties.json` the same way and picked up when each chat starts
  again. So is a `luna_spawn` the core went in the middle of: the agent is told no helper came
  of it, with its prompt in a file and any worktree it had begun named and left where it is,
  or with `resumeOnRestart` Luna starts it afresh and says so.
- The log also says how each run ended. Luna names its own reasons for leaving (tray
  Quit, an update, Windows signing out or shutting down). A small watcher process logs
  the exit code when something else ends it, which tells a kill from outside (Task
  Manager, `taskkill`, `Stop-Process`) from a crash, and notes whether Task Manager was
  open. If even the watcher is gone, the next launch reports when the previous run was
  last alive, whether its window had frozen first, and whether the PC restarted in between.

## Agents: sessions that run sessions

A chat created with **Luna tools** ticked (off by default) gets Luna as an MCP server, and
with it twelve tools: `luna_list`, `luna_sessions`, `luna_spawn`, `luna_send`, `luna_answer`,
`luna_read`, `luna_wait`, `luna_kill`, `luna_delete`, `luna_guide`, `luna_notify`, `luna_power`. The model can start a helper session — another model, another
account, Codex instead of Claude — with an opening prompt, wait for its reply, send
follow-ups, answer what it asks, and delete it when done, worktree and its `worktree-*` branch included if it asked for one.
`luna_notify` lets it raise a desktop notification mid-turn, titled with its chat's name and
opening that chat when clicked, at most one every 10 seconds. `luna_power`, for when the user
asks for it, arms the same shut-down, hibernate or sleep "when done" as the power chip, which
shows it and cancels it. The
tools and a short instruction cost about 2k tokens of context once per session; the full
account of sessions, accounts, models and tools is `luna_guide`'s, read only when the model
asks for it. A chat without the box ticked knows nothing of Luna at all, and neither does a
background helper spawned without tools.

- A helper runs **in the background** unless the agent asks for `visible: true`: no row,
  no pane, no terminal — each turn is one run of the CLI's headless mode (`claude -p`,
  `codex exec`), and the next one resumes the conversation. It needs no open window, and it
  goes by itself once its answer has reached the agent and it has sat idle for 30 minutes
  with nobody reading it or writing to it (see `autoDelete` below). Its parent's row
  shows how many there are, and the count glows for a few seconds when one is spawned; a
  click lists them with what each is doing and for how long, its model, effort, account and
  worktree, and opens one as a chat of its own (between its turns, once nothing they left
  runs), stops it or deletes it.
- A visible helper is a normal chat row, nested under the one that made it and folded by
  default; the count on the parent row unfolds it. It is not seated in a pane unless you
  open it. Its own status, context and title work as for any chat.
- What a session waits on comes with it: `state` says `working`, `waiting` (a permission),
  `question`, `idle` and so on, and `ask` carries the question with its options or the
  command up for permission — Claude Code's `AskUserQuestion` and permission prompts,
  Codex's questions and approvals. `luna_answer` answers it: an option's number or label or
  words of its own, `allow` true or false, or raw keys for any other dialog. A background
  Claude Code helper puts its questions and permission prompts to Luna instead of a screen
  (`--permission-prompt-tool`), so its agent answers them all the same; a background Codex
  helper has nobody to ask and says what it would have asked in its reply, unless it runs
  with `approval: auto-review`, where a reviewer model answers.
- A prompt or message longer than 8,000 characters reaches a visible session as a file it
  is told to read; a background one gets any length on its stdin.
- Codex under `read-only`, `read-only-net` or `workspace-write` (and so `auto-review`) runs its commands in its Windows sandbox
  (the unelevated kind: no administrator, no extra accounts), without the Store's app
  aliases on its PATH, which the sandbox cannot start.
- `read-only-net` and `danger-full-access` have the network. `read-only` and `workspace-write`
  do not by default, but on Windows Codex only points the proxy variables at a dead port, so
  what ignores them (ssh, raw sockets) still connects: that keeps tools off the network by
  accident, not data in. `read-only-net` is Luna's own value: Codex's read-only with the
  network on, as a permission profile passed for the run and never written to the account's
  `config.toml`; the profile's name is new on every start, so no config can have rules waiting
  for it. `auto-review` cannot use it, being `workspace-write` only. On Windows, TLS through
  schannel (the built-in `curl`, `Invoke-WebRequest`) fails under the sandbox's token, and so
  does Git's bundled ssh; `gh`, Python and plain http work. So does git over https, because
  Luna adds `http.sslBackend=openssl` to `GIT_CONFIG_*` there, after any you set and unless you
  chose a backend that way, so it trusts Git's CA bundle rather than Windows' certificate store.
  For git over ssh set `GIT_SSH` to Windows' own `ssh.exe`.
- `luna_spawn` can make a checker: `readOnly: true` takes Edit, Write and NotebookEdit from
  Claude Code, whose Bash can still write, and puts Codex in `read-only`, or in
  `read-only-net` if asked. `agent: "<name>"` runs Claude Code as an agent the project or
  account defines (`--agent`); Codex has no such thing for a whole session and refuses it.
- `account` in `luna_spawn` can be a list of one provider's accounts: the first signed in
  with every usage window under 95%, else the one with the most room. The result's
  `accountChoice` says which it took and why, and why it skipped the others.
- Every session gets its own id as `LUNA_SESSION_ID`, and a helper its parent's as
  `LUNA_PARENT_ID`. A Claude Code helper's system prompt also says what it is, and that it
  writes in the language of its task (`--append-system-prompt`). Codex can only replace `developer_instructions`, which would
  drop what its config says there, so each prompt of a background Codex helper (a visible
  one's first) ends with a note from Luna instead: a prompt that opens with a slash command
  goes without it, and a compaction can lose a visible one's.
- An agent kills and deletes only sessions it spawned (and theirs). It can see every other
  one, yours included, with `luna_sessions` — name, window group, account and its folder,
  project folder, cwd, the CLI's session id and transcript path — type a message into any
  of them with `luna_send`, and read or wait on the answer, so you can tell one chat to pass
  word to another. `luna_sessions` narrows to one group (`group: "mine"` is the caller's
  own) and to rows with some text in them (`q`), so the chats you put in one group can be
  told to settle something between themselves — who gets the machine next, say. Into a chat you are typing in, `luna_send` does not type: Luna holds the
  text and types it in once you stop. Over an unsent draft in a Claude Code chat it puts the
  draft aside (Ctrl+S, the CLI's own stash), sends, and the CLI puts the draft back, pastes
  and images and all — unless a draft is stashed already, which another Ctrl+S would
  replace. Codex has no stash, so there the text waits for the draft to be sent or cleared.
  A chat with an unsent draft wears a **draft** tag in its row and title bar, with the count
  of messages Luna holds for it, and a balloon over the pane says how many wait; a minute's
  hold is announced on the desktop. At most eight helpers under
  one chat, and a helper's helpers get no tools of their own.
- Every account is available to agents unless you switch it off with the robot button on
  its row in the Accounts window. `luna_list` names the ones that are, so you can tell a chat which one to use
  (the account alone is enough: it brings its own CLI),
  and gives each its models, the settings a helper would get there (model, effort, permission mode —
  read from the account's and the project's settings the way the new-chat dialog reads
  them) and its usage as the Accounts window shows it: percent per window and when it resets.
- `luna_spawn` says what the helper came up with that its settings cannot: warnings the CLI
  put on screen (a model the organization does not allow, swapped for another) and the other
  live sessions working in the same directory (`sameFolder`), whose files and git it shares —
  `worktree: true` keeps it apart, and the spawn then names the worktree the CLI made and its
  branch (`luna_list` shows them for every helper). An unknown slash command comes back at once
  as not started, with the screen that says so.
- `autoDelete` has Luna delete the helper once its answer has reached the agent and it has sat
  idle for 30 minutes; the agent reading it or writing to it starts that over, and `luna_list`
  shows the minutes left (`deletesInMin`). It waits for a job the helper left running and for
  helpers of its own; processes it left that have done nothing for 2 hours on end do not
  count: they go with it, and its agent is told so in one line. It is on for background helpers unless the spawn says
  `autoDelete: false`, off for visible ones unless it says `true`. A helper stopped with
  `luna_kill` is never auto-deleted: a background one's conversation stays for `luna_send` to
  go on with, a visible one is not running until you resume it. One
  you have typed into is yours and stays; a worktree it made stays too, with its work.
- `luna_send` with `interrupt: true` cuts short the turn the session is on, and any prompt it
  is stopped at — Esc in a terminal, the process stopped for a background helper — and the text
  becomes its next turn instead of waiting behind the current one.
- Every spawn is written down in `agents.jsonl` in Luna's data folder — when, by whom, where,
  account, settings, the start of the prompt — and how it ended, so `luna_sessions` with
  `history: true` can show what ran before, deleted helpers included, and a run can be repeated as it was.
  A deleted helper leaves its transcript path (the whole prompt is there), its worktree and its
  last message, of which `history` gives the first 400 characters.
- `luna_delete` with `dropWorktree` takes the worktree the CLI made and its `worktree-*` branch;
  a branch the helper named itself stays. It refuses when that would lose uncommitted changes,
  untracked files or commits no other branch or remote has, and says which, unless the agent
  adds `force: true`. A worktree that cannot be removed is named back to the agent as
  `worktreeLeft`. Your own Delete asks you instead, as ever.
- Deleting a parent by hand leaves its helpers as orphans, marked ↳ at the top level,
  unless you tick the box to take them along. Helpers deleted by their agent are gone for
  good, transcript aside; what those had spawned passes to that agent. A background helper
  with no parent left is listed under "Helpers with no chat" at the end of the chat list, to
  stop, open or delete, and goes by itself half an hour after it is done.
- `luna_read` reads the transcript, not the screen; `luna_wait` returns the reply itself —
  everything the helper said since it was asked, the report first — for an agent that needs
  it in the same turn (past 60,000 characters either is cut: `luna_read` says how many
  messages it left out, `leftOut`; with `since` alone the next read goes on from its cursor,
  with `last` the newest messages are kept; `luna_wait` names a file with
  the whole answer). Otherwise the agent spawns and ends its turn,
  and the answer comes to it:
- An agent that asked something and ended its turn without waiting is not left idle: once
  the answer comes — or the helper asks something, stops on a permission prompt, or exits — Luna types a
  one-line `[Luna notice, not typed by the user]` into the agent's chat, as soon as that
  chat is between turns and you have not typed into it for 20 seconds; a subagent it left
  working in the background (Claude Code) does not hold the notice back. A draft you left in
  it is put aside for the notice the same way (Claude Code), or holds it (Codex; a minute's
  hold is announced on the desktop). A
  `luna_wait` still out for that answer — Claude Code moves a call past two minutes to the
  background — hands it over instead, and the notice follows only if the wait has not done
  so within 10 seconds. A wait the agent's CLI cancels or interrupts hands nothing over,
  and the notice comes as usual. Text sent to a visible session mid-turn may join that turn
  or start the next one, so Luna gives the CLI 8 seconds after a turn ends to start another
  before it takes that turn as the answer.
- The notice is one short line: what happened, how many messages the helper wrote since it
  was asked, the first line of its report, and a file with all of them — the report (the
  longest message, or the last when every one is short) first, then the
  last one, then the rest — so a closing "that was a stale wakeup, nothing changed" does not
  hide the report before it, and a dozen "still running" lines do not hide the "DONE" after them. A `luna_read` collects the answer only when it returns all of
  it, uncut, and the helper is done: one while it is still at work, or one with only the
  last message of several, does not count, and the notice still comes.
- A turn that stopped on an error (a usage limit, an API error, a crash, a kill) is reported as
  that, with the reason, not as finished; `luna_wait` has it as `failed`. What the helper said
  before it stopped comes with it, as it does for a turn a restart of Luna's core cut short. A
  helper whose CLI quit before its model did anything (an unknown `agent`, a model that is not
  there) is idle, not busy with what that CLI had started, and goes like any other.
- A helper that ended its turn with a job still running, or helpers of its own still to answer
  it, is announced as not done once. The next notice comes when that is over or it ends a turn
  of its own: a turn it only ran on a Luna notice does not count, unless the notice was its own
  helper's and that was the last answer it waited for. So two chats with a stray process each
  do not keep waking each other. While its helpers are still out, what a helper makes of their
  answers reaches its own parent when it is more than a line or two (a message of 400
  characters or more), once: that only ever goes up from helper to parent, never back.
  A background Claude Code helper's own background job (`run_in_background`) keeps its turn
  going: `claude -p` waits for the job and takes its end up itself, so the helper stays
  `working` until then and there is one notice, with everything it said.
- What a session's turns left running past their end — a server, a watcher, a `tail -F` a
  tool forgot — is named: in its notice, and in `luna_list` as `leftRunning`, each process
  with its pid, when it started and whether it has used CPU or I/O in the last 10 minutes.
  For a background helper `luna_kill`, or Stop in the helper's list on its chat's row, ends
  it once the turn is over. When all of it has done nothing for 30 minutes, the agent owed
  the session's answer gets one notice saying so — it may be stuck, or waiting on something
  — and how to end it; the notice of its end still comes, as doing nothing is no proof of one.
- A turn that has written nothing for 30 minutes — no output from a background helper, no
  transcript line from a chat — gets the agent owed its answer one notice per turn: a command
  in it may hang, or just take long. The usual notice still comes when the turn ends.
- If a chat still gets more than 12 turn-end notices about one session in 15 minutes, Luna
  stops telling it of that session's turns and jobs, says so in the log and once on the desktop,
  and goes on when you type into either chat. Its prompts and questions still come: a helper is
  never left on a prompt until it times out.
- Luna types into a terminal one thing at a time, notices, sends and answers alike, so nothing
  lands inside a paste or in the way of a draft coming back. A send that cannot get its turn in
  time is held like one a draft is in the way of.
- A background helper's prompts each have an `id` in `ask`, which `luna_answer` takes as
  `askId`: an answer written on one cannot land on the next, and an id it never had is refused
  as unknown. Without it, an answer is taken only when one is held.
- `luna_wait` also takes several sessions (`ids`, `mode: "any"` or `"all"`) and says which
  finished. `luna_list` shows, for each helper, when its turn began, when it last did
  anything, the full model id it ran on (none until a model has answered) and what it spent: tokens, plus the cost where the
  CLI states it (Claude Code does, Codex does not). Visible sessions show less: Codex its
  token total, Claude Code none. An agent also gets a notice when an account its helpers run
  on reaches 90% and then 95% of a usage window, once each until that window resets.

## Power

The power field in the status bar opens the two rules the machine follows.

- **Keep the PC awake while sessions work** (on by default). Luna holds a Windows power
  request — `powercfg /requests` lists it as Luna's — while any session is busy, and lets it
  go a minute after the last one goes quiet. A session waiting for an answer — yours, or a
  background helper's question or permission its agent is to answer — does not hold the
  machine. What a session started in a turn, a chat's or a helper's, holds it after the
  turn until it has done nothing — under 1% of a core and 1 KiB of I/O a second — for 2 hours
  on end: a client waiting on work done elsewhere is quiet too, but a forgotten `tail -F` or a
  server nobody uses is let go then, and stops keeping the "when done" rule below waiting; the
  log says so, and it still shows in `luna_list`. Windows does not restart its
  sleep timer for the request: if nobody has touched the PC for longer than that timer, it
  sleeps the moment the hold is let go. `luna.log` says when the hold was set and let go, and
  which session held it last, and every hour it is kept, which process keeps it.
- **When every session is done: shut down, hibernate or sleep.** Armed for the run, not
  saved. The chip shows what the rule is waiting on; once everything has been quiet for the
  chosen window it counts down a minute with a notification, and a click on the chip cancels.
  Any activity during the count cancels it too and leaves the rule armed. Shutdown asks each
  CLI to quit first; sleep and hibernate leave the sessions running, and the same rule applies
  again after the wake. While the rule is armed and something holds it up, `luna.log` says
  which session and why, so a morning with the PC still on can be read back. An agent asked
  to turn the PC off when done arms this same rule with `luna_power`.

"Busy" is measured, not guessed, from three signals per session: the turn (Claude Code's
own hooks and registry, Codex's rollout), the processes under the CLI (put in a Job Object at
spawn, so a build, a test run or a `sleep` it left in the background counts as long as it
moves or was started by a turn; what the CLI brings up with itself — MCP servers, its shell —
only while it moves, and a program with a window of its own, like a browser a login opened,
not at all), and recent output. A background helper's leftovers between turns count the same
way. Deleting a chat or a helper, or quitting Luna, ends whatever it left running, windowed
programs aside; off Windows, where nothing tells a window from a job, it runs on. A core that dies takes a helper's whole job with it, a CLI still in its turn
included, so nothing runs on unowned; there the OS spares no window. "Done" means no session is busy *or*
waiting for a permission. A session whose turn ended on a usage limit (or another API error:
a lost login, a billing problem) and sits on the dialog it left counts as done — nothing moves
there before the reset — and the countdown notification says how many there were. Arming
the rule while a session waits for your answer brings a notification naming it, and the chip
reads `· N waiting for you` for as long as that holds the rule. To hear the turn boundaries, Luna starts every Claude Code session
with a `--settings` file that adds a few hooks posting to a loopback listener; they print
nothing, so nothing reaches the model's context.

## Development

```sh
pnpm install
pnpm dev          # frontend only, in a browser (tauri commands are no-ops)
pnpm tauri dev    # the whole app (needs a Rust toolchain)
```

## Screenshots

```powershell
./screenshots.ps1
```

Rewrites `docs/screenshots/*.png` in Docker: the frontend alone, standing on the invented
chats and accounts in `src/dev/demo.ts` (`pnpm dev` + `?demo`), photographed by a headless
Chromium. Shooting a running Luna would mean shooting whatever is open in it, and the
fixture is also the only way to retake all three shots from the same state.

## Checking the Rust side without a local toolchain (Docker)

```powershell
./linux-check.ps1
```

Builds the crate for Linux and runs its whole unit test suite there: the Unix code
(`keeper.rs`, parts of `procs.rs` and `pty.rs`) that `./test-build.ps1 -Test`, with its
Windows binaries, never compiles. The checkout goes in read-only, and the caches (the cargo
registry, the Linux target) live in named volumes of its own. Runs under `pwsh` on any host
with Docker.

## Windows build through Docker

```powershell
./build-windows.ps1
```

Builds the toolchain image (rust + cargo-xwin + NSIS + node) and cross-compiles for
`x86_64-pc-windows-msvc`:

- the exe — `src-tauri\target\x86_64-pc-windows-msvc\release\luna.exe`
  (self-contained; it only needs the system WebView2), for trying a build in place;
- the NSIS installer — `...\release\bundle\nsis\Luna_*_x64-setup.exe`, the one way Luna
  is shipped. Install it into any folder, on any drive: Luna keeps its data there.

The caches (node_modules, the Windows SDK, the cargo registry) live in named volumes, so
repeat builds are fast.

## Releases

```powershell
./release.ps1              # release the version in tauri.conf.json
./release.ps1 -Notes "..." # with your own release notes
./release.ps1 -DryRun      # build and compose the manifest, upload nothing
```

Bumping the version means editing `src-tauri/tauri.conf.json` and `package.json` together;
the script refuses to run if they disagree, if the tree is dirty, if HEAD is ahead of
`origin/master`, or if the tag already exists — a release has to be reproducible from a
pushed commit, and the build stamp in the binary is only worth anything if it is.

It builds the signed installer, writes the `latest.json` the updater reads, and uploads
the setup, its `.sig` and the manifest to a `v<version>` release. No bare exe goes up: the
updater installs a release into the folder Luna was installed in, so a copy run from
anywhere else would update into a second Luna with none of its data.

## Updates and signing

- On startup the app checks `latest.json` from GitHub Releases (the endpoint lives in
  `src-tauri/tauri.conf.json` → `plugins.updater.endpoints`). A release found there lights
  up a chip in the status bar and waits to be clicked, rather than throwing up a modal
  over whatever was mid-turn. Nothing installs without that click. A release whose Rust
  side is the same as the running core's (`core_fingerprint` in `latest.json`, a hash of
  `src-tauri` written by `build.rs`) restarts only the window: the sessions keep running in
  the core and the new window picks them up. Any other release stops them: the dialog lists
  them, and once the installer is downloaded every one is asked to quit, each with its exit
  hooks, and the chats resume from their transcripts afterwards.
- The CLIs update on the same principle and separately: `cli.rs` checks each one's release
  channel every six hours and installs into `<data>/claude-cli/versions/<ver>/` or
  `<data>/codex-cli/versions/<ver>/`. Claude Code comes as a bare exe from the official
  release bucket, verified against its manifest's sha256. Codex comes from GitHub Releases
  (`openai/codex`, tag `rust-v<ver>`) as the package tarball — the only asset with a line
  in the release's `SHA256SUMS`, and the one that carries the helpers the Windows sandbox
  needs next to `bin/codex.exe`. Claude sessions run with `DISABLE_AUTOUPDATER=1` and Codex
  accounts are created with `check_for_update_on_startup = false`, so Luna is the only
  thing that moves either binary.
- All three versions live in Settings (the gear in the status bar), each with its "checked …
  ago" and a check-now button; the status bar itself only speaks up while there is news —
  an update on offer, a download running, or a failure worth a retry.
- The update signing key is `C:\Users\Nikita\.ssh\luna-updater.key` — private and
  passwordless, so keep it safe; its `.pub` half is already baked into the config. For a
  signed build, set `TAURI_SIGNING_PRIVATE_KEY_PATH` in the build environment (which is
  what `build-windows.ps1` mounts) — `.sig` files then appear next to the bundle, and
  those plus `latest.json` are what you upload to the Release.
- Authenticode-signing the exe (so SmartScreen stops complaining) needs a certificate.
  When there is one, add it under `tauri.conf.json` → `bundle.windows.signCommand`.
