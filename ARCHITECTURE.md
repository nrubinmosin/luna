# Architecture

A desktop wrapper around the Claude Code CLI and the Codex CLI. Principle: **all session state lives in the CLI itself**;
the app only multiplexes terminals, folders and accounts. So the Rust core is small, and all the
presentation logic is on the frontend.

Two providers, two independent layers (`claude/` and `codex/`, both in Rust and on the frontend). What they
share is only the pty, finding and sweeping worktrees, the accounts folder and the mechanics of downloading the binary. Everything about
settings, limits, trust and reading the session from disk is each one's own and in its own terms: Codex does not
know the words "permission mode" and "Fable", Claude does not know "sandbox" and "rollout".

## Stack

- **Tauri 2** — Rust core + system WebView (light binary, native access to pty/fs).
- **React 19 + TypeScript + Vite** — frontend.
- **Zustand** (+ `persist` in localStorage) — UI state: chats/folders, pane layout.
- **xterm.js + FitAddon** — terminals in panes.
- **CodeMirror 6** — the hand editor for account settings files.
- **portable-pty** (Rust) — ConPTY on Windows / openpty on Unix, one code path for all platforms.

## Layers

```
src/                          frontend (feature-sliced)
  app/        App (sidebar dock: pinned or slides out on hover), AppFrame + BoardTools
              (layouts, groups, Even out, Clear, theme — in the window title bar), shell.store
              (theme, sidebar pin and width), theme.css (light/dark tokens, code colours
              --code-key/--code-string/--code-literal, and all controls),
              keymap: Ctrl+N / Ctrl+Shift+N, Ctrl+1..4 (focus pane), Alt+1..4 (peek),
              Ctrl+Shift+W (next waiting), Ctrl+Shift+B (sidebar), Ctrl+Tab (next group),
              Ctrl+Shift+T (new group),
              Ctrl+0 (exit), Ctrl+Shift+1..4 (layout), F1 (shortcuts) — listened to in the
              capture phase, otherwise xterm swallows them before window
  shared/     types, format, status; ui/icons.tsx — XP icons (and provider badges),
              ToolButton, title bar glyphs; ui/ModelSelect.tsx — model drop-down (native
              <select>); ui/dialogHost.ts
  features/
    chats/    sidebar: folders + chats (drag source, click = show), chats.store;
              worktrees.store + WorktreesDialog (stale worktrees, the title bar's Worktrees);
              drafts.store — a running chat's unsent draft and the messages Luna holds for it
              (the row's and title bar's `draft` tag, the pane's balloon);
              marked.store — which waiting chats wait by the marker, and clearing one by hand
              (a check on the pane's title bar and with the row's rename and delete)
    panes/    1/2/3/4 grid, Pane (drop target), Terminal (xterm), panes.store
              (auto-tiling lives there too: TILE_ORDER, board grows/shrinks by ±1 pane);
              peek — a sheet over the dimmed board, the same pane without remounting;
              altPeek.ts — a look at a chat while Alt is held over its row;
              terminals.ts — registry of live xterms that outlive their panes
    new-chat/ creation dialog: folder, account (which also picks the provider), checkboxes
              "Git worktree" and "Luna tools" (start state — from Settings or saved for the
              folder, Folder.start), for Codex "Hooks unasked" (the same, kept by the Rust
              side: codex/hooks.rs), then the provider's fields (Fields from
              providers/*) — they open on what *_defaults say; the model is a drop-down
              with the one the files (or the CLI) would pick marked "· default"; draft.ts —
              settings draft with a "touched by hand" mark, and `follow` for values that
              track another pick (effort follows the model); create.ts — the shared
              creation path for the dialog and for Ctrl+Shift+N
    providers/ claude/ and codex/ — everything the UI knows about each CLI: STOCK, defaults,
              dialog Fields, title bar Meta, account row LimitBars, login, trust,
              worktreeRe, projectOf(cwd) (Claude: from the path; Codex: null — the path
              names the account, not the project), branchPrefix (Claude `worktree-`,
              Codex null); index.ts — ui(provider) for the little that is shared
    accounts/ Accounts window (from the status bar): limits, sign-in, agents, add/remove;
              RecommendedDialog (recommended settings, per-account button), ConfigEditor
              (hand editor for settings.json / config.toml); accounts.store
    library/  Skills & instructions (from Accounts, or the status bar's chip while a copy needs a
              look): the library's folder, a grid of instructions against the accounts (a radio
              per account column for the one it gets, or none; ReachPicker per row: all, a CLI's,
              by name), a grid of skills against the
              accounts each goes into (ReachPicker, and an or/and switch once it is ticked for a
              folder) and each folder whose sessions get it (FolderTick: ticked, turned off, or a faint
              tick for what they get anyway; AddFolder — the new-chat dialog's folders, or the picker),
              "Needs a look" with differences (diff.ts — a line LCS);
              LibraryEditor — instructions/<name>.md / SKILL.md in CodeMirror, saved and laid out at once;
              library.store — the overview, a pass at startup, a look on every window focus
    settings/ Settings (gear in the status bar and in Accounts): accounts folder, auto-tiling,
              new chats, deletion, Luna/Claude Code/Codex versions and update buttons
    status-bar/ full-width strip of the window: working / waiting (click — to the waiting one), limits of all
              accounts, Accounts, power, settings, help; the update chip — only when there is news
    power/    power field in the status bar and menu: keep-awake, arming "shut down/sleep when everything is done";
              power.store listens to power://state, computes nothing itself
    shortcuts/ the F1 / "?" window in the status bar: all shortcuts and mouse gestures in one list
    agents/   bridge.ts — the frontend half of agents: agent://spawn → createChat with parentId
              and ensure_* with the prompt → agent_spawned; agent://deleted → deleteChat;
              agent_resumes → startSession (terminals.ts) for a helper to carry on, in a pane or not;
              a model asked for without an effort gets that model's default effort;
              agentAccounts.store — which accounts agents may not use (core settings.json)
  ipc/        commands.ts (invoke), events.ts (listen) — the only boundary with Rust

src-tauri/src/
  provider.rs   enum Provider { Claude, Codex } — a tag, nothing more
  accounts.rs   list/create/delete folders in <accounts root>/{anthropic,openai}/<name>;
                a Codex account gets a seed config.toml on creation
  settings.rs   <data>/settings.json: accountsRoot (default Documents/luna-accounts),
                changed in Settings; written under settings.json.lock re-reading the file,
                the cache is checked against mtime — more than one process may write the file;
                a file that will not read or parse is an error, never the defaults: the cache
                keeps what it read last and a write refuses
  emit.rs       the only door for events into the UI: the receiver is set once (the window in setup),
                sending code does not hold an AppHandle; pty::manager() — the process's only
                PtyManager, also without Tauri State
  cli.rs        shared mechanics of Luna's own CLI copies: versions/<ver>/, current, sha256,
                progress, prune, auto-update every 6h; where to get a version from — the
                provider's Source
  pty.rs        session manager: shared spawn in a pty, scrollback buffer, pty://output
                and pty://exit events, write/resize/kill; ensure_claude_session / ensure_codex_session
                build the command line; session_meta dispatches by provider;
                ActivityProbe::working_dir() — where a session actually works (Claude
                registry / Codex rollout)
  worktree.rs   finds the worktrees both CLIs make: Claude's under <folder>/.claude/worktrees
                (branch worktree-*), Codex's by shape …/worktrees/<4 hex>/<name> plus a .git
                file pointing back at the folder; removal, orphan sweep (including each
                account's worktrees/), fetch origin before a Claude Code worktree (setting
                fetchBeforeWorktree)
  throttle.rs   cool-off after a 429 from the usage endpoints, shared by both
  procs.rs      what the CLI has running under it: a Job Object per session, pid list,
                CPU/IO/start time of each, the "busy" rule (judge), windowed programs left
                out (shown), what is left ended with the session (end_leftovers)
  keeper.rs     the tie to the core off Windows: the `--keep-ties` process that ends a dead
                core's jobs, what a job holds there, the next core's backstop (end_last_ties);
                and `kill_tree`'s walk there (end_tree)
  browsers.rs   which browser a terminal's link opens in: a command line with %1 per
                browser; found on Windows (StartMenuInternet + each Chromium browser's
                Local State profiles, Firefox's profiles.ini, the https UserChoice handler)
                and the user's own; chosen chat → account → folder → default, run with the
                link as an argument of its own and no shell (open_link in lib.rs)
  hub.rs        loopback HTTP (tiny_http) on a random port with a per-launch secret;
                POST /hook/<chat>/<secret> takes Claude Code hooks' stdin;
                POST /mcp with the session's bearer token — MCP for agents; a thread per request
  mcp.rs        the MCP server's JSON-RPC: initialize (with instructions), tools/list, tools/call;
                twelve luna_* tools with short descriptions (~2k tokens), offered to sessions given
                them; luna_permission only to a background Claude Code helper's CLI
  agents.rs     registry of sessions with tools and those they spawned: tokens, kinship,
                rights (kill/delete — descendants only; sessions, send, read, wait — all), spawn via the frontend
                (agent://spawn ↔ agent_spawned), send into the pty, read from the transcript, wait on activity, kill, delete (+worktree);
                notifier: an uncollected helper answer is typed to the caller when it is free;
                autoDelete helpers are deleted once they have served
  background.rs helpers with no row (luna_spawn without visible): a turn is one run of
                `claude -p --output-format stream-json` or `codex exec --json`, prompt on stdin,
                events on stdout, the next turn a resume; a held permission / question
                (`--permission-prompt-tool`) waits for luna_answer; agent://background to the window
  agentlog.rs   <data>/agents.jsonl: a line per spawn and per end, for luna_sessions(history: true)
  activity.rs   sampler every 5 s: turn (hooks + registry / rollout), descendants, output →
                busy / waiting / idle per session and a summary
  asking.rs     a user's chat whose turn ended on the marker setting → `waiting` in
                session_meta until the next turn, or until cleared by hand (dismiss_waiting)
  power.rs      keep-awake (PowerSetRequest) and "shut down when everything is done":
                arming, quiet window, countdown, action; commands and the power://state event
  claude/
    cli.rs      Source: downloads.claude.ai, latest + manifest.json, bare exe
    defaults.rs model/effortLevel/permissions.defaultMode from Claude Code's settings in its
                order: the account's settings.json → the project's .claude/settings.json →
                .claude/settings.local.json → machine-wide managed
    limits.rs   OAuth usage + CLI cache (.claude.json); 5h, week and each weekly_scoped
    oauth.rs    refreshes an expired access token the same way the CLI does (its locks, CAS)
    models.rs   context windows from the Models API
    trust.rs    hasTrustDialogAccepted in .claude.json; a file that will not read or parse is
                read again and then an error, never taken as empty and written back
    session.rs  registry <config>/sessions/<pid>.json + transcript projects/<cwd>/<sid>.jsonl:
                context, CLI title (custom-title / ai-title), first prompt
  codex/
    cli.rs      Source: GitHub Releases openai/codex, rust-v<ver>, package tarball + SHA256SUMS
    config.rs   config.toml via toml_edit (comments and formatting kept), seed; a read for a
                write looks again at a file caught blank, cut short or held open, and one that
                will not parse is an error, never an empty document written back
    models.rs   model catalogue, sorted by priority, with per-level descriptions
                (effort_hints): the account's models_cache.json → another Codex account's →
                `codex debug models` → `codex debug models --bundled`; models_known — without
                running the CLI, models_for — with the CLI fallbacks
    defaults.rs model / model_reasoning_effort / approval_policy / sandbox_mode from
                the account's config.toml, then the project's .codex/config.toml (trusted only);
                no model there = first model of the catalogue (source codex), no effort =
                that model's default_reasoning_level
    limits.rs   chatgpt.com/backend-api/wham/usage via auth.json; fallback — rate_limits
                from the last token_count in the rollout
    trust.rs    [projects.'<path>'] trust_level = "trusted" in config.toml; or for one run,
                `-c projects={"<checkout>"={trust_level="trusted"}}` (trust_override), which writes
                nothing: every background helper's turn, and any session in a project under a folder
                of settings.json `codexTrustedRoots` (Settings → "Codex trusts on this computer").
                Codex matches trust by the checkout's exact path, never a parent folder's
    hooks.rs    whether a session runs the project's hooks unasked (`--dangerously-bypass-hook-trust`):
                the default and per-folder choices in Luna's settings.json, where the core reads them
                for helpers — background ones (`codex exec` leaves untrusted hooks out silently) and
                visible ones (the spawn request carries it) alike. A visible Codex stopped on a list
                of its own ("Hooks need review") is an `ask` (agents.rs `codex_list_on`)
    session.rs  rollout files sessions/YYYY/MM/DD/rollout-<ts>-<uuid>.jsonl: which is ours,
                status, model, context, first prompt, cwd (rollout_cwd — where a --worktree
                session went); thread name from session_index.jsonl
  account_settings.rs  recommended settings (settings.json + .claude.json / config.toml, key by
                key, platform-aware; apply_recommended adds and removes keys), sync from the
                parent account, and hand editing of the file (check_account_config: problems
                with line/column; format_account_config: JSON pretty with 2 spaces, TOML via
                taplo, comments and key order kept; only what parses is written). settings.json
                and .claude.json, which the CLI rewrites in place, are read again when caught
                half-written or held open; one that still will not parse is an error, not an
                empty object written back
  library.rs    the user's skills and instructions: a library folder (`<data>/library` or one of
                the user's) with `instructions/<name>.md` and `skills/<name>/SKILL.md`, laid out into the
                accounts each is meant for — a reach of `all`, `claude`/`codex` or `provider/name`
                entries (`CLAUDE.md` / `AGENTS.md`, one instructions file per account, the closest
                reach winning; `skills/<name>`) — and skills for the sessions in folders the user
                names, handed out (or kept from them) at every start by `session_args` (pty.rs,
                background.rs); a ledger
                of what Luna wrote where (`<data>/library-state.json`, bytes in `library-blobs/` by
                hash) so only its own copies are touched — see Key decisions
  watchdog.rs   a window whose page stopped responding (WebView2 died) is rebuilt
  coreproc.rs   the core: the same exe with `--core`, no window; one per data dir (core.lock), listens on
                127.0.0.1 with a token from <data>/core/core.json, exits by itself after 10 minutes with no
                window and no sessions (a background helper in a turn is one, and one whose turns left
                work that still holds the PC, `background::running`; in the welcome's count too);
                leaving, it stops every helper's turn with its process tree (`background::stop_all`); on Windows it runs from a copy of the exe <data>/core/<fingerprint>/,
                so the installer can replace luna.exe. pty, hub, agents/mcp, activity and power
                live here; events go to all windows through a per-window queue (a window that falls
                behind is disconnected, the pty does not wait for it, and the window goes back to the
                core at once, its panes reattached from the scrollback: corelink.rs `rejoin`)
  corecmd.rs    commands the core answers: the page calls them as before, the window forwards
                (core_call / core_send for input), here they map by name onto the same functions
  corelink.rs   the window side: find the core of its own build or start it, hello, hold the connection
                (its loss = the core died → banner, Restart); a core of another build with no sessions
                is replaced at once, one with sessions — only on a button; a core.json whose pid is
                dead is skipped without connecting; restarts run one at a time; core events → webview
  ipc.rs        frames `u32 len | u8 kind | JSON` (REQ/RES/EVENT/SEND), hello/welcome,
                discovery file, token
  lifecycle.rs  why Luna stopped: its own exits with a reason, the `--watch-exit
                <pid>` watcher (exit code: killed / crashed), luna.run with the last sign of life
                for the next launch, event loop hangs and machine sleep; the tray's Restart
                window (a new window started as `--after <pid>`, the core left running)
  paths.rs      data_dir(): next to the exe when writable, else LOCALAPPDATA\luna — settings,
                log, attachments, CLIs and (Windows) the WebView profile; the test
                build (LUNA_FLAVOR=test) keeps everything next to its exe in test-build/
  lib.rs        Builder + generate_handler
```

## Key decisions

- **Thin Rust core.** Rust does not know about "chats" — only pty sessions by id and account folders.
  The chat list, layout, settings — persisted frontend state. Smaller IPC surface,
  easier UI evolution.
- **Own CLI binaries.** Luna does not depend on a global `claude`/`codex`: `cli.rs` keeps copies in
  `<data>/claude-cli/versions/<ver>/claude.exe` and `<data>/codex-cli/versions/<ver>/bin/codex.exe`
  (`<data>` is the exe's folder wherever that is writable, so everything lives on one drive), `current`
  points at the active version. Versions are never overwritten (a running exe cannot be
  replaced on Windows) — a new one goes next to it, the pointer switches, old ones are swept once nobody
  holds them. Claude sessions get `DISABLE_AUTOUPDATER=1`, Codex accounts get
  `check_for_update_on_startup = false`, so only Luna updates the CLI. Until there is a copy —
  fallback to PATH. Codex comes as a package tarball (the only asset with a line in
  `codex-package_SHA256SUMS`, and it carries `codex-command-runner.exe` and the other Windows
  sandbox helpers), unpacked whole into the version folder.
- **A Claude session = running `claude`** with arguments:
  `--model <alias> --effort <low|medium|high|xhigh|max|ultracode> --permission-mode <mode> [--resume <uuid>] [--worktree [<name>]]`,
  `cwd` = the chat's folder, `CLAUDE_CONFIG_DIR` = the account folder (login/settings isolated per account).
  A worktree chat whose worktree is already known continues from the project folder as `--worktree <name> --resume <uuid>`
  (as the CLI itself advises on exit), not by starting inside the worktree: on exit Claude Code silently removes
  a clean worktree (no changes, no commits) — and every session exits on each Quit and Luna update —
  and given the name the CLI makes it again, with the same name and branch. The core does not start a folder
  that does not exist at all: portable-pty would silently substitute the user's profile.
- **A Codex session = running `codex`** with arguments:
  `[resume <uuid>] -C <cwd> --no-daemon [--worktree] --model <m> -c model_reasoning_effort="<e>" -a <on-request|never>
  -s <read-only|workspace-write|danger-full-access>` (the pair `never` + `danger-full-access`
  is replaced by `--dangerously-bypass-approvals-and-sandbox`; Luna's `auto-review` is `--approve-for-me`
  in place of `-a`/`-s`, which Codex refuses beside it, in `workspace-write`), `CODEX_HOME` = the account folder.
  `--no-daemon` keeps the session in this process, in Luna's pty, rather than in Codex's shared background
  server — which the `-c` overrides rule out anyway; said outright, Codex does not warn about it at startup.
  Worktrees are Codex's own: a new worktree chat starts with `--worktree` and `-C` = the project folder,
  and Codex puts the checkout at `<CODEX_HOME>/worktrees/<4 hex>/<repo>` (inside the account folder) on a
  detached HEAD, no branch. Luna learns the path from the rollout's `session_meta.cwd`
  (`codex/session.rs` `rollout_cwd`), the same way it learns Claude Code's from its registry
  (`useSessionWatch`). Codex refuses `--worktree` on resume, so a worktree chat resumes as
  `resume <uuid> -C <worktree>`; if that worktree is gone the session fails to start — nothing
  re-creates it. Login is the same command with `login: true`, i.e. `codex login` in the account folder.
- Flags are always set, so the CLI's own settings would be overridden on every
  session — `claude/defaults.rs` and `codex/defaults.rs` read them themselves and put them into the dialog, so
  by default the flags match the files, and a divergence happens only where it was asked for
  by hand. Codex always gets `--model`: `codex_defaults` resolves it as config.toml's `model` or else
  the first model of Codex's catalogue (sorted by `priority`; source `codex`), and the effort as
  config.toml's `model_reasoning_effort` or else that model's `default_reasoning_level`. Picking
  another model in the dialog moves the effort to that model's default, unless the effort came from
  a settings file or was picked by hand (`draft.follow`). What Codex actually ran is read from the
  rollout's `turn_context` (`modelSeen` on the chat).
- **An account = a folder** `<accounts root>/anthropic/<name>` or `<accounts root>/openai/<name>`
  (default `Documents/luna-accounts`, the root is chosen in settings and kept in Rust — the
  background thread of `models.rs` reads it too). Create — mkdir, delete — rm -rf, list — readdir over
  the two subfolders. No database of its own. Chats remember the provider and the account name, so changing
  the root needs no migration — just move the folders.
- **A Codex session on disk.** Codex has no live registry, only the thread's rollout file
  `<CODEX_HOME>/sessions/YYYY/MM/DD/rollout-<local time>-<uuid>.jsonl`, a line = JSON
  `{timestamp,type,payload}`. Which file is ours: the thread the TUI says it is on — Luna starts it with
  `-c tui.terminal_title=["thread-id"]`, and the pty reads the window title (OSC 0/2) off the output, which no
  pane shows; 0.166 cuts the item to 29 characters, which leaves 25 hex digits of the uuid, matched against
  the start of the one in the name (two alike are no answer) — once written. That follows a `/new`, `/clear`, `/fork` or `/resume`
  in the TUI, whose new thread keeps the file it was on until its own is written, but not `/subagents`:
  the title then names the subagent watched, whose rollout says `{"subagent":…}`, and the session stays on
  its own (seen with 0.166: it moved onto the subagent's and counted its turns); the turn count goes on
  over every file the process has been on, each read once, and what agents are owed is read on from the
  start of a fresh transcript or the end of one gone back to (agents.rs `moved_to`, Claude Code's `/clear`
  and `/resume` too). Before the title has said: for a fresh session — the earliest of those that appeared
  after the spawn whose `session_meta` cwd is the folder, or a Codex worktree pointing back at it; for a
  resumed one — by the uuid in the name; the answer is remembered per chat, so two chats in one folder
  do not share a file. Only a rollout whose `session_meta.source` is `cli` can be guessed: a background
  helper's says `exec` and a subagent's `{"subagent":…}`, in the same folder and at the same moment, and being
  on the file that holds after a helper is deleted or the core restarts, with no list of ids to keep.
  Claude Code's registry has the same trap by another door: `claude -p` keeps a `<pid>.json` too while it
  runs, so past the pid and the session it was told to resume, the fallback to cwd + start time takes only
  `entrypoint: cli`. Read on incrementally from where the last look stopped (a turn outgrows
  any tail: a megabyte of tool output after `task_started` read as resting):
  `task_started`/`task_complete` (before 0.157 — `turn_started`/`turn_complete`/`turn_aborted`) →
  working/resting and the turn count, `request_user_input` (holds the turn) and
  `request_user_input_async` (open until the next `UserMessage`) → the question asked. From the
  tail (128 KiB): `token_count.info` → context (`last_token_usage.total_tokens −
  reasoning_output_tokens` against `model_context_window`, as Codex itself counts it),
  `turn_context.model` → model, `token_count.rate_limits` → limits fallback. The conversation itself —
  `response_item` lines of type `message` with `role` user/assistant (developer messages and
  mixed-in context like `# AGENTS.md instructions` and `<environment_context>` are not the conversation).
  Approval requests are not written to the rollout, so "waiting" for Codex is a turn in flight with a
  screen that has not changed for 3 s (Codex draws its spinner continuously). The rollout appears only
  with the first turn: a session sitting on the start-up popup has no file at all.
- **Worktrees are the CLIs'.** Each CLI makes its own from `--worktree`; Luna never makes one, it
  only finds them again — to remove one with its chat, or sweep the ones no chat claims. Claude Code's
  live under `<folder>/.claude/worktrees` on a `worktree-*` branch; Codex's are recognised by shape
  `…/worktrees/<4 hex>/<name>` plus a `.git` file (`gitdir: <repo>/.git/worktrees/<id>`) pointing back at
  the folder (`worktree::is_worktree_of`, `project_of`); every destructive path is gated on this. `project_of` goes by that
  `.git` file for Claude Code's too, its path's shape saying only where to look: a folder under a worktree is of it if no
  repository of its own lies on the way down and git does not ignore it (`in_checkout`), so a test project in a worktree's
  `test-build` is no worktree of the main checkout, and a helper sent there makes its worktree where it was sent. One gone
  from disk goes by where it was, as Claude Code removes a clean one on exit and makes it again by name. The
  orphan sweep (the title bar's Worktrees window, every folder at once) scans each account's `worktrees/` as well as the project's. Removing a Codex worktree also
  removes its bucket dir once empty; only `worktree-*` branches are ever deleted (Codex's checkouts have
  none). Where a session actually works is `pty::ActivityProbe::working_dir()` (Claude registry / Codex
  rollout); delete, `worktree_shared`, `live_cwds` and `agents::caller_worktree` go by it, the worktree's root
  from anywhere in it (`worktree::root_of`: Claude Code moves its cwd with its shell's `cd`). A background
  helper has no pty to probe: `background::working_dirs` gives the folder it was sent to and the worktree
  it made (its first `init`'s cwd; a later one, of a turn `claude -p` takes up by itself, says where the shell
  went, and a cwd saved that way comes back as the root, `home_of`), between turns too, and `worktree_shared`, `live_cwds` (the orphan sweep) and `account_in_use`
  (account rename) count it as they count a running session.
  `fetch_origin` (Settings "Fetch origin before Claude Code makes a worktree") applies only to Claude Code: Codex checks out the commit the folder is on, which no fetch moves, and Luna never moves the user's own checkout.
- **Scrollback in Rust.** The output buffer (2MB cap) lives in the core, so that moving a chat between panes
  or recreating the xterm restores the screen (`ensure_session` returns the backlog).
- **A terminal outlives its pane.** A pane takes a ready xterm from `terminals.ts` and returns it to
  "parking" (off screen, but in the document — otherwise the renderer and the fit addon measure zero),
  so switching the chat in a pane costs no `ensure_session` with the backlog, reset and wait for the CLI
  to redraw — what switching chats costs in single-window mode. The three most recently put away wait
  there with all their subscriptions; beyond that the oldest is evicted. Returning = refit plus a nudge
  to redraw.
- **Events instead of polling (pty).** pty output streams as the `pty://output` event; termination —
  `pty://exit` (the chat goes idle).
- **Account limits — zero tokens.** `claude/limits.rs` reads the OAuth token from
  `<account>/.credentials.json` and calls `api.anthropic.com/api/oauth/usage` — the same endpoint
  as `/usage` inside Claude Code; the CLI's cache in `.claude.json` answers while it is fresh.
  `codex/limits.rs` reads `tokens.access_token`/`account_id` from `<account>/auth.json` (email and
  plan — from the `id_token` claims) and calls `chatgpt.com/backend-api/wham/usage` — the same as
  `/status` inside Codex; the `primary`/`secondary` windows plus `additional_rate_limits` are returned as a
  labelled list (300 min → "5 hours", 10080 → "week"). If there is no network or token — the last
  `token_count.rate_limits` from the rollout. The frontend polls every 60s; ↻ in the account row asks at once,
  past the CLI's fresh cache and the cool-off. The cool-off after a 429 is shared (`throttle.rs`).
- **Claude token refresh.** An access token lives ~8 h, and an account that has not been opened for a while
  would hang on "waiting for token refresh" until its first session. `claude/oauth.rs` exchanges the refresh token
  itself — the same `POST platform.claude.com/v1/oauth/token` with the CLI's client_id and scopes. Refresh tokens
  rotate, so only by the CLI's rules: both its mkdir locks (`<config>/.oauth_refresh.lock` and
  `<config>.lock`, stale after 60 s), re-read the file under the lock, write only if the refresh token
  on disk is still the one that was sent. A CLI that comes to refresh after us sees a different
  access token on disk and takes it (its own "race resolved" branch). `invalid_grant` = a new
  login is needed, the account is shown signed out. Codex is left alone: its token lives 10 days, it refreshes
  itself, and the server rejects an already used refresh token ("refresh token was already used. Please
  log out and sign in again") — our refresh next to a live Codex that remembers the old token would sign it out.
- **Chat titles come from the CLI.** After the first message Claude Code generates a session name (the same
  one it sets on the terminal tab) and writes it to the transcript as an `ai-title` line; `/rename` writes
  `custom-title`. Both are rewritten into the tail as the session goes on, so they are read together with the context.
  Order: `custom-title` → registry name with `nameSource: user` (both are a manual rename) →
  `ai-title` → first prompt (only while there is no title yet). A registry name with `nameSource: auto` and
  the second `ai-title` the CLI writes after a plan is accepted (the plan's kebab handle, also repeated by
  the `agent-name` line) do not count as a title. For chats without a live session `saved_title` finds the
  transcript by session id once per launch. For Codex — the lines with that id in
  `<CODEX_HOME>/session_index.jsonl`: it writes the first one itself, anything that differs from it is `/rename`.
- **A chat is named once.** `Chat.nameSource` says where the name came from, and `mayRename` decides what
  may replace it: a typed first line (`prompt`) — only the placeholder; the CLI's first title
  (`cli`) — only the placeholder or the prompt, after that it is frozen; `/rename` in the CLI (`rename`) — everything
  except a name from Luna; a name from Luna (`user`) is final.
- **Trust.** Neither CLI can show its trust prompt the way Luna runs it (Claude
  Code refuses under `--worktree`, Codex after it asks about the Windows sandbox),
  so the bit is written before the spawn: `hasTrustDialogAccepted` in `.claude.json` or
  `[projects.'<native path>'] trust_level = "trusted"` in `config.toml` (via `toml_edit`, so as
  not to lose the user's comments; the key is compared case-insensitively and with either slash).
- **Session busyness is measured, not read from one status.** Every 5 s `activity.rs`
  combines three signals: the turn (for Claude — hooks via a `--settings` file with `curl.exe` (outside Windows — `curl`) to the loopback
  `hub.rs`, checked against the registry; for Codex — the rollout plus screen silence, and an approval dialog
  (a command, an edit), which is not in the rollout and under which the screen is not silent, — by its text on
  the rendered screen; the same verdict also goes to the chat row in the UI instead of `working`), the CLI's descendants
  (`procs.rs`: on spawn the CLI is put into a Job Object, descendants inherit it; "busy" = growth in
  CPU/IO over the window, or a process born after the first turn began and still alive — that is how
  a background `sleep` is caught, while what the CLI starts in its first 30 s (`STARTUP_MS`: MCP
  servers, its shell) counts as furniture however soon a first turn begins, and a respawn starts that
  count over; a program with a visible window of its own, and everything under it, is the user's and
  is left out) and pty output over the last 30 s. The job sets no limits: when a session goes
  (delete, kill, Quit, a respawn of one that exited) `procs::end_leftovers` ends what is still in it,
  which the CLI's tree kill misses once a parent has gone first, windowed programs spared. A
  background helper has one job for its life, each turn's process joined to it: between turns what
  its turns left holds the machine until it has done nothing for 2 h on end (`LEFTOVER_HOLD_MS`), and it is
  ended with the helper. Doing something is measured per process (`procs::leftovers`): CPU at 1% of a core or
  1 KiB/s of I/O between two samples at least 4 s apart, as Windows counts CPU in 15.6 ms ticks (over a minute
  a `tail -F` polling its file used 0.5 ms/s and 250 B/s, a `ping` 1 ms/s and 100; an 8 KiB/s download is past
  it). Doing nothing is no proof a job is over — a Docker or remote client waits on work done elsewhere — so
  the grace is long, but it ends, with a log line: a Claude Code Monitor's `tail -F`, left running past its
  expiry, kept a PC awake all night. Birth is no work: a process first seen is as quiet as the job it came
  into, and until it is 30 s old only its CPU since it started can say otherwise (its I/O then is loading,
  which a read in the middle of its start would count next time), so a loop starting a `sleep 60` a minute
  stays idle. `luna_list`'s `active` is work in the last 10 min. A chat's are measured the same way (activity.rs `Track::left`, the processes
  `procs::judge` calls young): a forgotten `tail -F` under a chat held the machine just the same, and
  `judge` itself counts a child's I/O only past that rate, as a polling one moves a few bytes every
  sample. While the hold stands, power.rs says every hour what keeps it (`HOLD_LOG_EVERY_MS`). Its job alone
  is tied to the core (`procs::tie`, `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`): a chat's CLI goes with its
  terminal when a core dies, a helper's has none, and would run on unowned beside the turn the next
  core starts on that conversation. This is the leftover rule, not an exception to it: a core that
  leaves ends leftovers already (`stop_all`). The OS spares no window, so every way out that is not a
  death lets go first (`let_go`, `Drop for Job`), `end_leftovers` sparing windows as ever; a window a
  helper's turn opened dies with a core that dies, and only then. Off Windows the same tie is a process
  of Luna's (`keeper.rs`): the keeper, this exe with `--keep-ties`, one to a core once a helper has a
  turn, reads a pipe only the core writes, and the end of that input is the core's death, on which it
  ends every job not let go of; it also looks at the core itself every 2 s by pid and start time, as on
  macOS a pipe becomes close-on-exec a moment after it is made and a process started then holds the write
  end. Not `PR_SET_PDEATHSIG`: it fires as the spawning *thread* ends, reaches one child, and macOS has
  none. Such a job is more than a process group, since both CLIs start each tool command in a session of
  its own (Claude Code `detached`, Codex `setsid`): it is what has its mark in the environment (`LUNA_JOB`,
  one to a helper under a core), the turns' processes by pid and start time with their groups, and all
  below those by parent link. Codex's `shell_environment_policy` may keep the mark from its tools, so
  `CODEX_THREAD_ID`, which it sets after that filter, counts too for what was born since the job began;
  a turn whose own process does not show its mark still goes with the core but is not counted as tied.
  Everything is stopped from the top down before it is killed, and no signal goes to a group by its
  number: on Linux each goes through a pidfd. A core writes what it has tied as it ties it, a file to a job
  in `<data>/tied-jobs/`, so that letting go of one is one unlink a full disk does not refuse, and the next
  ends what is left of those before it takes the helpers up (`end_last_ties`), for a keeper that went with
  its core. A pid whose start time differs is another's and is left alone, a file of another boot names
  nothing, and a job not seen out stays written down for the core after. One difference: a job there lists nothing, as nothing there tells a
  window from a job left running, so `children_busy` stays false off Windows and what a turn left runs
  on past every way out but a death. `kill_tree` is `taskkill /T` there by the same walk without the mark
  (`keeper::end_tree`): the process, what it started, and what is in a group or session one of those
  leads, each stopped from the top down and looked below again so that nothing forks out, then killed. So
  `luna_kill`, an interrupt, a core's stop and a chat that would not quit take the tool commands with them,
  a program with a window too if it still hangs from the tree, as `/T` takes it; what only the mark ties to
  the job has outlived its tool and is left. A chat's CLI that quits when asked is gone by then: what is
  left of its group goes, and what it left elsewhere stays. In the core a live pid that is no child of the
  core is another program's and is left alone. Hooks print nothing: Claude Code adds the stdout of a hook exiting 0
  to the context, and silence is free. All but the guard (`guard.rs`): a PreToolUse hook on
  `Bash|PowerShell|Monitor` for a chat with tools and for every helper (a background `claude -p` gets it
  alone), and for Codex `-c hooks.PreToolUse=[…]` on its shell, which its hooks call `Bash`, where hooks run
  unasked (untrusted, `exec` drops it and the TUI stops on a review screen). Codex runs a hook through the
  user's shell, PowerShell on Windows, where a bare `@-` does not parse: the hook failed, and a failing hook
  lets the call go ahead. Its command has `"@-"` quoted and curl by full path, as PowerShell, cmd and sh all
  take it. It posts to
  `/guard/<id>/<secret>`, apart from `/hook` and its locks, which answers a command that follows a file with no
  `timeout N` in front of it (`tail -f`/`-F`/`--follow`, `Get-Content -Wait`; quotes, comments, heredoc bodies
  and `bash -c` read as a shell would) with `permissionDecision: "deny"` and what to do instead, and
  anything else with an empty 204. A tool's own `timeout` is no deadline: past it Claude Code moves the call
  to the background. With the hub down the call goes ahead. Claude's registry beats an "idle" from a hook, a hook's
  "waiting" beats the registry's "busy" (a permission prompt is inside a turn). No mark is eternal:
  a hook's "busy" against the registry's "idle" goes stale in 15 s (a missed Stop), a hook's "waiting" — at
  once when the registry's idle is newer than the hook (its `statusUpdatedAt`: a prompt answered "No"
  ends the turn with no Stop), else in 60 s (the prompt left with the turn on Esc, Stop never came),
  the registry's "busy" against a hook's "idle"
  — in 60 s without a single hook (a turn that began or did something marks itself with a hook).
  Without hooks (Codex, a session that survived a Luna restart) the registry is believed as is.
  The registry's `waiting` beats a hook's "busy": the dialog is open, the model is not working (a turn cut off by
  an API error sends no Stop — it sends `StopFailure`, which Luna also subscribes to: end of turn
  plus the kind of error). A session waiting on a dialog after such an error (by `StopFailure` or by
  the last transcript line with `isApiErrorMessage`) is *stalled*: it does not count as `waiting`.
  A subagent's hooks carry its `agent_id` and are never the turn's: a background agent works on after
  the turn that started it has ended, firing a hook per tool, and Claude Code's registry says busy all
  that while (`isLoading || delegatedActive`). So Luna keeps the subagents at work apart, from
  `SubagentStart` (or a first tool) to `SubagentStop`: while any is, the registry's "busy" against a
  hook's "idle" is theirs and the turn reads over at once — notices go in — yet the session holds
  the machine like a job its turn left (`children_busy`, "N subagents" in the power chip), and a
  subagent's own prompt reads `waiting`. An idle registry that has stood 15 s says none is left, for a
  `SubagentStop` that never came. Claude Code takes an agent's result up in a turn of its own, which
  opens with a `UserPromptSubmit` (its prompt the `<task-notification>`) like any other (seen with
  2.1.318); after a `SubagentStop` the registry's "busy" is judged afresh all the same.
  Codex (0.166) runs its subagents inside the session's own process and tells nothing of them on its
  screen or in the session's rollout: each writes a rollout of its own, whose `session_meta.source`
  names the thread that spawned it (`{"subagent":{"thread_spawn":{"parent_thread_id":…}}}`). At each
  sample Luna reads those of a live chat's thread and of every thread spawned under it
  (`codex::session::children_at_work`): one is at work while the last turn it opened has not ended,
  and one done with its task stays open, asleep until it is written to, and is not. The set so read
  replaces the last (`agents_on_disk`), holds the machine and counts as the chat's job as Claude Code's
  do, and the 15 s rule, which is Claude Code's, takes none of it. Only threads begun since the process
  was spawned are looked at, in every day's folder since (a fortnight at most): not one of an earlier run
  woken again, nor the subagents of a thread the TUI has left (`/new`, `/resume`). Not a background
  helper's: `codex exec` follows its own thread only, and its subagents go with it when that turn ends.
- **A turn that ended on a question** (`asking.rs`) is waiting for the eye only: `session_meta` reads
  `waiting`, with `marked`, for a user's chat, no helper's, whose turn is over and whose last message (the
  transcript's tail, the rollout's `task_complete.last_agent_message`) ends on a line holding the marker
  from settings. The user may clear it by hand (`dismiss_waiting`, a core command: a check on the pane's
  title bar and the row's, which pass the key of the ending they showed); it stays cleared, in memory, while
  those words and the time they were said, from the same read of the transcript or rollout, are the ones it
  was shown on, so the next turn ending on the marker shows again, the same words included. A CLI's own prompt is never `marked` and cannot be cleared. The activity tracker keeps
  the turn idle, so power counts it as done.
- **Power follows busyness** (`power.rs`). Keep-awake — `PowerSetRequest(SystemRequired)`
  with a reason visible in `powercfg /requests`, while busy > 0, released after 60 s of quiet;
  `waiting` does not keep the machine up (nobody is answering), but blocks shutdown (the work is not
  done). Stalled (limit and other API errors) does not block: until the reset nothing
  will move there. Arming with `waiting` > 0 sends a notification with the chat names at once. Arming lasts until Luna exits; while armed the hold is always kept, otherwise the PC would sleep in the
  quiet window. The automaton: busy = 0 and waiting = 0 continuously for quiet_s → a 60 s countdown with
  a notification → action. Shutdown closes sessions via `shut_down` with a 5 s grace (the CLIs'
  exit hooks) and `shutdown /s /t 0`; restart (Windows only, refused at arming elsewhere) — a RunOnce value
  with the window's quoted exe (`--window-exe`: the core runs from a copy; a failure is logged and the
  restart goes on), `coreproc::wind_down` as a core leaving does, so a chat stalled on a limit keeps its
  mid-turn state, and `shutdown /r /t 0`; sleep/hibernate — `SetSuspendState`, sessions live on, arming stays,
  and the clock jump on wake resets the quiet window and the countdown. A drain (`Armed::drain`, "start
  nothing new") refuses `luna_spawn` and every `luna_send` in `mcp::run` alone, so nothing a person does is
  touched, and narrows `luna_answer` to a pending ask; each such call is counted under the lock that arms
  (`power::enter`), the arming waits for those already in (`settle`, bounded, and they hold the rule up until
  done), and firing ends the drain. The test build and unit tests never reach the OS (`power::dry`). An armed rule that
  does not fire is explained in `luna.log`: a line `armed, not firing: <chat> <why> [hook …,
  registry …]` when the reason changes (no more than once a minute) and every 10 minutes with no change;
  `luna.log` takes INFO too (log.rs): events only — session start/exit/delete, helper turns, one line per
  MCP call with its steering arguments and outcome but never its text, notices — each session named by id
  and chat name (`agents::label`), so a night can be read back after its sessions are deleted.
  A child born of a turn holds for as long as it is at work, and once idle for 2 h on end
  (`LEFTOVER_HOLD_MS`) no longer: a `tail -f` left behind and a `sleep 64000` before a check due
  tomorrow look the same from outside, and a PC let sleep or shut down on a guess stops work that only
  a person can start again, so the clock runs from when it last did anything, not from the end of
  the turn. The hold itself is logged both ways: `awake hold set: <who>` and `awake hold
  let go: … last held by <who>`. The request does not restart Windows'
  idle timer: once nobody has touched the PC for longer than its sleep timeout, it sleeps the moment
  the hold is let go, so that line is the time of the sleep.
- **Agents — via MCP, not via the prompt.** A session with the "Luna tools" checkbox gets
  `--mcp-config <data>/mcp/<chat>.json` with `"timeout": 1900000` on the server and `--allowedTools mcp__luna`
  (Claude: without `timeout` it drops an HTTP call that is silent for 5 min) or `-c mcp_servers.luna.url=… -c
  mcp_servers.luna.bearer_token_env_var=LUNA_MCP_TOKEN -c mcp_servers.luna.tool_timeout_sec=1900
  -c mcp_servers.luna.default_tools_approval_mode="approve"`
  (Codex: by default it gives an MCP call 60 s, while `wait` holds the request up to 30 min; Luna's tools
  ask no permission in any mode, the checkbox is the permission) — the Luna server on the loopback
  `hub.rs`, a token per session, reissued on every spawn. The agent's instruction travels in the
  `instructions` of the `initialize` answer, so the system prompt is touched neither by a flag nor by a
  file; all of it together is ~2k tokens once. Plain sessions get neither MCP nor any knowledge of Luna.
  A helper, tools or not, is told what it is, and to write in the language of its task (an account's
  instructions may set the user's language for their chat). Claude Code: one line by `--append-system-prompt`. Codex: a
  note after each turn's prompt of a background helper (a visible one's opening prompt only; `luna_read`
  gives the prompt back without it), since `-c developer_instructions=` replaces that key and its value comes
  from a stack of config files Luna would have to resolve as Codex does (ancestors' `.codex` included) and
  could then overrun a command line. Slash-command prompts go without; a compaction can lose a visible
  one's. Every
  session has `LUNA_SESSION_ID` in its environment, a helper `LUNA_PARENT_ID` too. A helper that passes to
  another parent (`pass_up`) is told as it next starts: a background one's next turn takes both from
  `Spec::parent`, a Codex one's note with it; a chat's next start goes by the core's record (`parent_on_start`), as a running process keeps what it has.
  One left with no parent is told of none.
  The core does not know chats, so a spawn is a round trip: `agent://spawn` to the frontend, which makes the chat
  (`parentId`, not seated in a pane) and `ensure_*` with the prompt as the CLI's positional argument after
  `--` (Claude Code's `--mcp-config` takes any number of files and swallowed the prompt as one more),
  and answers `agent_spawned`; the core waits on a Condvar up to 90 s, and then up to 30 s more — until the
  session takes up the prompt (`started`); if it did not, the answer carries `screen`, and if the screen is still
  empty — `starting`: before its first draw Claude Code syncs the organisation's skills/plugins and
  makes the worktree, and that can take minutes; a chat's CLI started again after a restart is as blank, however far
  its count has gone (`still_starting` goes by this process's turns), and for its first minute a notice also waits for
  its prompt (`drawn_for_notice`). The prompt meanwhile sits in the CLI's arguments, and anything typed into the pty
  would be lost, so `send` to such a session refuses. `read` and `wait` on timeout also return `starting`. An event reaches only a page that is listening:
  `agent://spawn` sent with no window, or with one still loading, is gone, and the spawn would time out with a window right
  there. So the request stays on its record until a session comes of it (`SpawnTaken::asked`), and the page asks for what is
  waiting once it listens and at every core (`agent_spawns`), taking each once however it came. Its row is named by the request,
  so a page reloaded half way finds the same row and no second, and `ensure_*` starts nothing that runs. `read` —
  from Claude's transcript (user/assistant, without tool_result) or Codex's rollout (`response_item`
  message user/assistant) with a byte cursor (whole lines only); a result is held to 60,000 characters
  (`MAX_TEXT`; Claude Code cuts an MCP result past 25,000 tokens): read on from `since`, it keeps the
  oldest messages that fit and puts the cursor back where they end, with `last` the newest; `leftOut`
  counts the rest, and one message past the cap is cut with a pointer to the transcript.
  `wait(turn_done)` counts busy→idle transitions in
  `activity.rs` (`turns_ended`; the count grows across respawn/resume too — for Codex only turns after
  the process spawned are taken from the rollout) and returns everything the session said past the
  debt's transcript offset (below), report first as in the notice's file (`reply_of`) — past `MAX_TEXT`
  cut, with that file named — so `read` is not needed.
  After Enter, `send` waits up to 12 s for a turn to begin and also answers `started`/`screen`. `screen` is
  the pty scrollback run through a terminal emulator (`vt100`) at the pty's size: the last
  40 lines of what a person sees in the pane — a popup, an error, a question, which the transcript does
  not have; `read(screen: true)` returns it on request, `wait` — on `timeout` and `waiting`, unless told `screen: false`. `list` and
  `spawn` return `settings` — the flags the session actually runs with; for Codex `list`'s `flags` take
  the model catalogue from `models_known` (no CLI run). Rights: only
  the caller's descendants — except `sessions` (all sessions of this Luna launch: account and its folder,
  chat folder, cwd, the CLI's sessionId, path to the transcript/rollout, pid; the frontend sends chat names
  as `chat_names` on every change), `send`, which writes into any of them except its own, and
  `read`/`wait` — also on any except its own (the transcript is on disk anyway, and whoever wrote must
  be able to collect the answer); `kill`/`delete` — descendants only;
  tool depth 1; at most 8 descendants. Children outlive the parent (`forget`): each passes to the parent's own parent, which reached it already through
  that one (a visible one's row hears, `agent://reparent`, as it names its parent to every spawn, a core started later included); with none it is an orphan (`Known::orphan`, saved in `Ties`): no agent has a claim on it, a visible one stays a row with ↳ (a child in another folder also stands apart, but with the parent's name
  in the tooltip), and a background one is listed under "Helpers with no chat" (`LooseHelpers`, any helper whose line up through helpers ends at no chat row) and joins `auto`, since no row shows it and nobody awaits its answer; a helper started without a folder works where its caller does, worktree and all
  (`agents::caller_worktree`, both providers). Deleting a child via `delete` removes its worktree if asked — `remove -f -f`,
  because Claude Code locks its worktree and a killed one does not release the lock; the worktree is where the
  session actually works (`working_dir`), and the project for the removal is derived from the path
  (`worktree::project_of`); a worktree another session or background helper works in stays
  (`worktree_shared`), whether the one deleted is a chat or a background helper.
  **A helper does not report on itself** — unlike a subagent, its answer sits in its
  transcript until someone calls `wait`/`read`. So `spawn`, `send` and `answer` record a debt
  (`owed`: caller, session, `turnsEnded` at the time of the question, and where the transcript ended
  before the text went in — taken before delivery, as a short turn is over and written by the time the
  call returns; 0 for a spawn, whose whole transcript answers the opening prompt). Asked again while a
  debt is open, the caller is owed both: the new baseline, the earlier offset (`min`), so the next reply
  covers both. Only an outcome that carries the answer settles it: a `wait` ending `turn_done` (or `exit`
  after such a turn) past the baseline; a timeout or another outcome hands nothing over and only
  restarts the autoDelete clock (`looked`). A `read` settles it only if it began at or before the
  debt's offset, returned everything from there uncut (not a `last` that left some out, not a result
  `MAX_TEXT` cut), ends on an assistant message, and the session was idle with nothing of its own
  running before the read began — a read mid-work has seen a turn on the way (a background helper's
  turn that ended on a job, the turn before a queued `send`), and settling on it would leave the real
  answer unannounced and the helper auto-deleted unread. The default `afterTurn` of a `wait` is the
  open debt's baseline, else the count last shown (`seen`): a count shown since the question (part of
  the answer read, a timeout) does not make the answer seen. A `send` to a background helper mid-turn owes the
  turn after the running one: the queue starts it the moment that one ends. Text typed into a visible
  session mid-turn is the CLI's to place — into the running turn or a turn right after it — so for such
  a debt (`queued`) a turn end counts as the answer only once no turn has begun 8 s later
  (`QUEUED_GRACE_MS`), for `wait`, `read` and the notice alike: a read settles such a debt once a
  turn past the running one has ended or the turn end has stood 8 s (`queue_settled`). A notifier thread looks at debts every 2 s and, when the caller is between turns
  on two looks in a row (`Idle`, not starting, the person has not typed into it for 20 s), types
  one line to it `[Luna notice, not typed by the user] …` with Enter — just like `send`: the helper
  finished a turn, stopped on a prompt (once per stop) or exited. The answer is everything the helper said
  past the debt's transcript offset (`replies_since`), not the transcript's last message: a helper's own
  stale wakeup ("nothing changed") ending a later turn must not stand in for the report. The notice quotes
  only the report's first line, about 150 characters on one line (`opening_line`), and not of a turn that
  stopped on an error, whose first words are no verdict on it: the answer goes to
  `<data>/agents/<id>-<sha256, 128 bits>.md` — named by what it holds, so two answers never share a file and a notice
  a draft holds back writes none anew at each look, with its time refreshed for the week's sweep when it is named
  again — the report first (`report_of`: the longest message, or with every one under 400 characters the last, as
  a Codex helper's dozen "still running" lines would bury its "DONE"), then the last, then the rest in order — and the notice gives the count of messages and the path. The
  same notice carries `quota_news`: an account a caller's live helpers run on at 90%, then 95%, of a usage
  window as `account_usage` last showed it, once per level per window; told only once the line has gone in,
  forgotten when the window resets, kept in `<data>/quota-told.json` across cores. `wait_many` (`luna_wait`
  with `ids`) is one `wait` per id in a thread of its own. Those that finish stay owed, in `helpers.json` too,
  reserved from the notice (`Reserved`) however long the others take; all are handed over together when the
  response is made, and a cancel crossing it undoes them together, except an answer a later call has delivered
  since (`receipts`): a core going or a cancel before that leaves the notice to bring them. `luna_list` adds
  a `task` (the prompt's first line, `Registry::tasks`, set in `spawn_is` as a session comes of the spawn, however
  late, and kept in the ties as `agents.jsonl` is cut under a live helper), `branch` and `sha` (`worktree::head_at`: off the `.git` files through any symbolic refs, no process
  per row) and
  `Signs` per row: for a background helper from its event stream (`Observed`, saved in `helpers.json` but for
  the last event's moment), for a visible one from the activity track and its transcript. Its `context` is
  read for either off the tail of the transcript or rollout, as the pane's meter reads it (`Context::of`). A
  Codex helper's model comes from the rollout's `turn_context`, or for that turn from the `model rerouted` item
  a reroute leaves in the stream, and only of a turn its model got to: the rollout names the model a turn is to
  run on before asking it. Claude Code's comes from the model's messages, not `init`, which names the one asked for, there or not;
  where the rollout names none, the one asked for stands only once a model has answered (`Observed::answered`,
  saved). A visible Codex's is the last `turn_context` after which the thread's token total grew
  (`answered_model`): a count is written again unchanged when rate limits renew. A
  caller handed a cut quote read the whole answer back anyway, and a long notice reached Claude Code as a
  paste. Typing a notice takes seconds, and the turn it starts may ask again meanwhile: the notifier
  then changes the debt only if it is still the one it announced (same baseline and offset).
  A turn after which the helper still has processes running waits up to 20 s: they went quiet — "finished",
  they did not — "NOT done yet", once per debt (`Owed::told_pending`; a question asked since is a new debt). From there
  on the caller hears of a turn only if no notice started it (`notice_turn`, `real_turn_past`), or of the job's
  end (`Verdict::JobOver`, 20 s of nothing running and no turn, as its processes come and go between samples and Claude
  Code may take the end up in a turn of its own; Codex and a finished `claude -p` will not). A `claude -p` with a
  `run_in_background` task open does not finish: it waits for the task and takes its end up itself in the same run,
  so a background Claude Code helper reads `working` all along and its caller gets one notice, with everything it
  said. The "NOT done yet" names what a session's turns left. Once all of it has done nothing for 30 min
  (`IDLE_JOB_MS`) the caller is told once (`Owed::told_idle`, `Settled::Idle`) that only idle processes are
  left, named, that they may be stuck or waiting, that `luna_kill` ends them, and that their end still comes:
  idleness is no end, so the debt stays open and `luna_wait` gives no `job_done` for it. Before that a helper
  whose Monitor left a `tail -F` read "not done" for 14 h with no word to its caller. That line, and a job's end
  with no word from the session, no error and no core gone with it (`Settled::NoNews`), are no reason for a wake
  on their own, a full read of the caller's context: they ride with the next notice it gets (`riders_go`). Alone
  they go once they are all it is owed, or nothing anywhere is busy (`Summary::busy`; all a power-off's count starts
  on, and it knows no debts), or after `RIDE_MAX_MS`, as a caller may be waiting on that very job while another
  runs for days. Their debts stay open meanwhile, so `luna_wait` and `reap` see them as ever, and news
  replaces the line. Let out is not heard: a draft that holds a Codex caller's notice back starts no turn, so a
  power-off goes ahead of it, as it does of any notice held so. Turns on notices are
  skipped over, the cursor staying put, so what they said comes with the next word. This is what ended a night of
  460 notices an hour: every notice started a turn, every such turn's end re-owed the answer at the new count, and any
  leftover process made `children_busy` true. A debt that was not told this cannot loop: a notice consumes it.
  A helper with its own helpers still to answer it (`waits_on_helpers`) is held the same way, and its state reads
  `background`; a turn on a notice from them counts once it has all their answers, so what it makes of them reaches
  its caller, and a `wait` or `read` that takes the first answer leaves the rest owed (`shown`). While some are still
  out, its parent hears of turns on their notices alone that wrote a message of 400 characters or more
  (`news_for_parent`, `NoticeTurn::only_helpers`), each once, the cursor moving past it: a report its parent needs now
  ("kill the core before 12:35") sat there unread. That goes up the tree the helpers were spawned in and never back:
  two chats that wrote to each other are no parent and helper, and a helper hears nothing of its parent this way.
  A job's end is clocked on the debt itself (`Owed::held`): the notifier and a `wait` go by the same 20 s, the wait's
  own hold counts only from there (`held_for_wait`, `job_end_stood`), so the end reaches the caller once, and `wait`'s
  `job_done` brings what was said since as the notice does.
  Every debt has a revision (`Owed::rev`), and what is decided about one touches only that one: `take_debt` looks and
  takes in one hold of the lock, `settle_told` and a `job_done` go by the revision, so a question asked meanwhile stands
  even with the same baseline and offset.
  **The breaker.** Past 12 turn-end notices in 15 minutes for one (caller, session) the notifier stops telling that
  pair of that session's turns and jobs (`BREAKER_NOTICES`, `count_notice`, `pair_stopped`): the last notice says so, the
  log warns, the desktop is told once, and only a person typing into either chat lifts it, as two models answering each
  other could lift anything else forever. Prompts, questions and exits are neither counted nor silenced
  (`standing_line`): they hold until answered and cannot loop, a busy helper asks many, and one must not sit on a prompt
  until it times out. Real use peaked at 3 per pair in half an hour; the loop ran one a minute.
  Luna types into a terminal one thing at a time (`TypingInto`): notices, each on a thread of its own as `type_text` waits
  up to 12 s for the turn to start and one stuck caller delayed every other; held sends on `luna-held`; and `send` and
  `answer` in the tool call's thread, which wait up to 25 s for their turn and read the session only then. A send not let
  in by then is held, an answer is refused. A paste and its Enter 400 ms later, or a draft put aside and back, has
  nothing typed in between: that is Luna's writers, and `type_text` holds a person's keys back too (`hold_keys`) from
  before its look at the draft to the Enter, as landing there they would go out with the text; they go on after it,
  into an empty composer. `answer` does the same. A second Enter for one the CLI missed does not go over keys typed
  since, which it would send.
  A notice a draft holds back is worked out again at each look, so answer files are named by their contents.
  **A turn that failed.** `failure_of`: a background helper's `error` (its `result`/`turn.failed` event, a
  non-zero exit, `luna_kill`), Claude Code's last transcript word when it is an `isApiErrorMessage` line, else its
  `StopFailure` hook, Codex's `task_complete.error` or `turn_aborted` reason in its rollout (`Scan::failed`), and for a
  pty session its exit code. The notice says "stopped on an error", `wait` has `failed`. `isApiErrorMessage` lines are
  not messages (`message_of`), so "You've hit your session limit" is no reply; nor are the user lines Claude Code puts
  in itself (`injected`: a `<task-notification>` it takes up, an interrupt's marker, a compaction's summary, by
  `turnOrigin`/`promptSource` where the line says where it came from, which wins over its text, and by the text
  where it says nothing, as the interrupt marker does) or its `<synthetic>` replies, as Codex's own `<…>` items are
  not. A prompt last means the reply is still to come (`own_words`), but a turn cut short or failed has said all
  it will: it hands over what it said before that, whatever came after, in a notice and a `wait` alike, a turn a
  core's going stopped with no error of its own left (taken up a second time) as one too. Codex's `exec` says its retries as
  `error` events too ("Reconnecting... 2/5"), which are not failures. A Claude Code transcript is looked for under the
  registry's cwd, then in every project folder (`located_transcript`): a session resumed elsewhere writes to the folder
  it started in. No transcript is not "no reply": the notice and `transcriptMissing` say which.
  The CLI's own resident helpers (`codex-code-mode-host` and its
  `conhost`) do not count as background work while they do not move (procs.rs, `without_furniture`):
  otherwise every Codex helper was announced "NOT done yet" after a turn. A turn with no answer in the transcript waits up to 5 s for it (Claude
  Code's Stop hook arrives before the last answer lands in the transcript; `wait` waits the same way), and then
  the notification says "no answer" and points to the screen. Luna counts the draft from the person's keystrokes
  in `write_session` (`pty::draft_after`: characters add, Backspace subtracts, Enter,
  Ctrl+C, Ctrl+U reset, Enter right after `\` is a Claude Code line break, not a submit;
  key escape sequences print nothing, text inside a bracketed paste does):
  a notification with Enter would go out together with the draft. The counter only says there may be
  one: once the keys have settled (1.5 s), `has_draft` reads the composer off the screen
  (`pty::read_composer` — the cursor stands at the caret in both CLIs; Claude Code's `❯` between two rules,
  Codex's `›` down to the first blank row, its dim placeholder left out) and an empty one, or one of
  spaces, zeroes the counter: edits the keys do not tell (Esc Esc, Ctrl+W, a cut selection) no longer hold
  anything until the next Enter. A person's own Ctrl+S in Claude Code marks the session (`stashing`): the CLI brings
  that draft back unasked with the next prompt, so until the screen shows no stash the composer is read
  even with the count at zero. A real draft in Claude Code is put aside for the text (`stash_and_type`):
  the person's keys are held back (`hold_keys`, let go in order), Ctrl+S stashes the draft, the text and
  Enter go in, and the CLI brings the draft back as it takes the text ("Draft restored") — that return is
  the sign the text went, so there is no second Enter, which would send the draft. Not when the screen
  says `› stashed` above the composer: a second Ctrl+S on text replaces the stash, and the draft in it is
  lost. Codex has no stash. Where the draft stays, a notice waits for it; a `send` is held
  (`Registry::held`, `deliver_held` in the notifier, in order per session; `held: true` in the result),
  and so is one into a chat typed in within 5 s. A `wait` on a session with the caller's text held waits
  for it to go in, and then for the turn it starts, not the `afterTurn` given (`Owed::late`); a session
  that exits first is announced as such (`Owed::lost`). A draft or text held for a minute is announced
  once on the desktop. `session_meta` carries `draft` and `held` (`held_count`: sends waiting to go in,
  notices the chat's draft keeps back) for the chat row's and the title bar's tag and the pane's balloon. `wait(turn_done)` on a helper
  stopped on a prompt also returns (`waiting` with the screen): the turn itself will not end; for 8 s after
  a `send` from the same caller into such a session the prompt counts as answered. A notification is an ordinary turn of the caller; a dev caller without
  a terminal does not get them. While the caller's `wait` on that session is still running (`waits`), that
  `wait` delivers the answer: Claude Code moves an MCP call longer than 120 s to the background, but keeps the request, and the result
  arrives as a task-notification. A notification for such a pair waits, and if the `wait` has still not delivered
  the answer 10 s after it appeared (the client dropped the call), it goes out by itself. A `wait` its
  caller lets go hands nothing over and leaves the debt for the notice: a `notifications/cancelled` for its
  request (a cancelled request gets no response), the caller's session gone, or — a Codex caller, held
  in the call for as long as it lasts — the turn that made the call over (an interrupt) with no other
  begun, for 8 s on end (`let_go`, `LET_GO_MS`: a sample published late shows the turn before over);
  while that is unsure nothing is handed over. Each tool call in flight has an `mcp::Call` whose fate
  is decided once by `compare_exchange`: Running→Committed when a `wait` or `read` hands the answer
  over (`hand_over` settles the debt and returns an undo), Running→Cancelled on the cancel. A cancel
  that crosses the response finds the call committed — an answered one stays findable for 10 s
  (`LATE_CANCEL`) — and runs the undo: the debt (merged with any question since) and the count seen
  come back. A cancel never loses the answer; at worst it arrives twice.
  **What `spawn` says beyond the flags.** An unknown slash command: Claude Code writes `Unknown
  command: /x` and starts no turn — `await_start` sees the line on screen and answers `started:
  false` at once, not after 30 s. `warnings` — screen lines with ⚠ (a model forbidden by the
  organisation, and what replaced it: `settings` do not know that). `sameFolder` — live sessions
  whose cwd (per the CLI's registry) matches the helper's folder, except the caller itself; a helper in
  its own worktree does not get them.
  **Accounts in `list`.** Only when asked: `ids` narrows `list` to those rows, of any session, and takes the
  accounts off unless `accounts: true`, what a bare call pays for most. `defaults` — `claude::defaults::flags` / `codex::defaults::flags` for
  the caller's folder: the core does the normalisation and stock values (`source: stock`), the frontend
  only turns them into its labels, so the dialog, spawn and `list` say the same thing. `usage` —
  what the frontend shows in the Accounts window, sent as `account_usage` on every change:
  the core does not go to the limits endpoints itself.
  **autoDelete.** `reap` on every notifier pass: a helper with no debt either way, no `wait` on it, no helper of its
  own still there, nothing left running that still holds the machine (`left_holds`: leftovers idle past their
  2 h grace do not, and `remove` ends them; the debt open for their end is no bar, and is settled by one line to
  whoever held it, `reaped_with_leftovers`, `Registry::news`) and no turn (or one that exited) for 30 min in a
  row is deleted as via `delete`, without its worktree; to the log —
  `auto-deleted`. Any `read`/`wait`/`send` on it (`shown`) starts the 30 min over, and `list` shows what is left
  (`deletesInMin`). If the person ever typed into it (`typed_ago_ms`), or an agent `kill`ed it, Luna leaves it
  alone. Before any delete of a spawned session `remove` writes a `left` line to `agents.jsonl` — transcript
  path, worktree, last message — which `history` folds into the run, the last message cut to 400
  characters (`HISTORY_LAST` in mcp.rs: thirty runs at length overrun one result).
  **ConPTY and the cursor.** portable-pty opens ConPTY with `INHERIT_CURSOR`: until the terminal answers
  `ESC[6n` ConPTY lets out not a byte of the CLI's output. For a chat in a pane xterm answered, for a helper
  without a pane — nobody, and it hung on an empty screen until it was opened (`starting` for minutes).
  The core itself cuts each `ESC[6n` out of the stream and answers with the cursor position per `vt100` at
  the pty's size; the request does not reach the scrollback, so a pane that attaches later does not
  type a stray `[1;1R` into the CLI either.
  **Input to Codex — as key presses.** ConPTY turns each character into a key press on the console's
  layout, and a character that layout lacks (on Russian — square and curly brackets, `< > ' ~ @
  # $ ^ & |`, backtick, guillemets, dashes) never reached Codex — neither from `send` nor from
  the person's keyboard. For Codex the pty writer sends printable characters as win32-input-mode
  sequences (`ESC[0;0;<utf16>;1;0;1_` and the release; halves of a surrogate pair —
  press only). Ctrl with a letter goes the same way, as the Latin letter with Ctrl held
  (`ESC[67;0;99;1;8;1_` for Ctrl+C): from the bare control byte Codex asks the layout which
  letter the key is, and on Russian Ctrl+C was Ctrl+С, which neither copied Codex's own
  selection nor interrupted. Backspace, Tab, Ctrl+J, Enter and escape sequences go as is.
  Claude Code reads text and does not need this.
  **`read-only-net`** is Luna's value for what `sandbox_mode` cannot say: `-c default_permissions=
  "luna-<random>"` with that profile defined in `-c` too (`extends = ":read-only"`,
  `network.enabled = true`), in place of `--sandbox`/`sandbox_mode`, nothing written to `config.toml`.
  The name is new on every start because Codex lays a config's rules for that name over these: a known
  name would let a project's `.codex/config.toml` make it writable. On Windows it also gets
  `http.sslBackend=openssl` appended to `GIT_CONFIG_*`, so git's https does not go through schannel.
  It beats a `sandbox_mode` or sits beside a `[sandbox_workspace_write]` in the account's or project's
  config without complaint (checked on 0.166: writes denied, network up). `network.enabled` is direct
  network, not Codex's proxy (`features.network_proxy`). The other side of it: on Windows the "no network"
  of `read-only` and `workspace-write` is only env (`HTTP(S)_PROXY`, `ALL_PROXY` and git's at 127.0.0.1:9,
  `GIT_SSH_COMMAND=cmd /c exit 1`), not a block: ssh and raw sockets connect, and so does `gh` with those
  variables cleared. Under the restricted token schannel TLS fails (`SEC_E_NO_CREDENTIALS`) and Git's MSYS
  `sh.exe`/`ssh.exe` die (`CreateFileMapping`, error 5), with the network on or off.
  **The Codex sandbox on Windows.** Without `windows.sandbox` Codex rejects (`approval never`) or
  sends for approval (`on-request`) every command under `read-only`, `read-only-net` and `workspace-write` — even
  `Get-Content`. So those modes get `-c windows.sandbox="unelevated"` (a restricted token per
  command: no administrator, no sandbox accounts; under `workspace-write` Codex adds one ACL entry
  for its sandbox to the folder) and a PATH without `…\Microsoft\WindowsApps`: Codex takes the first
  `pwsh.exe` on PATH, and the Store PowerShell's app alias there fails under the restricted token
  with `CreateProcessAsUserW failed: 5` on every command (openai/codex has a row of issues on it);
  without it Codex falls back to an ordinarily installed PowerShell or Windows PowerShell. The
  instruction says that the CLIs' built-in subagents can do neither another provider nor another
  account: without that a Codex orchestrator took its own `spawn_agent`.
  **Background helpers** (background.rs) are what `luna_spawn` makes unless asked for `visible:
  true`: no chat row and no pty, all in the core, so no window is needed. Each turn is one process —
  `claude -p --output-format stream-json --verbose` / `codex exec --json`, resumed by the session /
  thread id the first turn printed — with the prompt on stdin (no command-line limit) and stdout
  parsed into a log of what it did (`luna_read(screen)` shows that in place of a screen); the
  conversation lands in the CLI's own transcript / rollout as with any session. Busy is the process
  running, not its stdout staying open: a job the turn left can have inherited that pipe, so it is read on a thread of
  its own (`OutReader`) and the turn ends with the process, once the reader has got through what the CLI wrote: it ended,
  or it has sat in a read with nothing coming for 500 ms (30 s at most). An event is looked at under the lock against the
  turn being over, so none lands after its turn. Between turns `children_busy` is what the turn
  left in the helper's job (`procs::left_in`, read as the turn ends and at each activity sample, as it takes a
  process snapshot), each process kept with when it was last at work (`Leftover`); `luna_list` shows them
  (`leftRunning`: exe, pid, start, `active`, `quietMin`), the window lists them on the helper's Stop, and a
  stop between turns (`luna_kill`, that Stop) has only them to stop and ends them, leaving `autoDelete` as it
  was; it holds the helper meanwhile (`Entry::ending_left`), so a send queues rather than start a CLI in the
  job being emptied, and starts the next turn once the stop is through. A turn whose CLI went before its model
  wrote or ran anything (`Observed::acted`: an unknown `--agent`, a model that is not there) first ends what is
  that CLI's by descent (`procs::end_tree_of`: up through parents in the job, each born since the turn began, to
  its pid); a way up that breaks, a launcher gone, proves nothing and is spared, as is all an earlier turn
  left: the `git` Claude Code had started ran on for minutes, and the helper read `background`
  and was never reaped; `send` during a turn queues the text for the next, and a turn about to start takes the
  helper's run under the same lock that found it free (`claim`), so text arriving before the process
  is up queues rather than starting a second one. A stop (`kill`, `stop_turns`) that finds a turn
  claimed but with no process yet marks the run `stopped`: `spawn_turn` does not start it, or kills
  what it just started. A turn that cannot start counts as one ended turn with its error, since whoever
  queued it waits on that end; text queued behind it fails with it, as the turn after. On Unix each turn's process leads a process group
  of its own, as a pty's CLI does. The core stops every turn
  as it leaves (`stop_all`, and no turn starts after), and `stop_all_sessions` before an update stops
  them too (`stop_turns`): a turn still running would keep the old core from being replaced. A Claude Code helper always gets Luna's
  MCP server, if only for `--permission-prompt-tool mcp__luna__luna_permission` (hidden from its
  model by `--disallowedTools`): its permission prompts and `AskUserQuestion` arrive as that call,
  held on a Condvar until `luna_answer` (allow / deny with the input back, or `answers` by question
  text) or the turn's end; several at once (parallel subagents) queue, and the oldest unanswered is the one shown. Each
  has an opaque id in `ask`, which `luna_answer` takes as `askId`: one no longer open is refused, one the helper never
  had (`Registry::asked`) is called unknown, and without it the
  answer goes through only with a single ask held, so a slower responder's `allow` cannot land on the next. Codex's `exec` asks nobody (approval never) and puts a question in its
  reply — or, with `approval: auto-review`, runs `--approve-for-me`, and a reviewer model answers. Helpers are auto-deleted unless spawned with `autoDelete: false`. `send` with `interrupt` puts the
  text behind the queue, kills the turn's process tree, and waits for its reader to count the turn over, whose end
  starts the next turn on the queue as with any other (`background::interrupt`), so no one's text hangs on how the kill
  goes: a turn that will not stop, or has no process yet to stop within 3 s, has it all behind it, and the
  result says `queued`; in a terminal it is Esc, up to 3 s for the turn to read as
  over, and the text typed as a prompt of its own. A spawn with `worktree: true` waits up to 20 s for the
  CLI to say where it made it (Claude Code's `init` event, Codex's rollout) and returns `worktree` and
  `branch`, which `list` carries for every helper too, or `worktreePending` if the CLI has not said by then. The window gets their list as `agent://background` (the
  count on the parent row) and can hand one to a chat row between turns, in two steps so that a row
  that cannot be made leaves the helper rather than neither: `background_promote` marks it (no turn
  starts, text sent queues) and returns what the row needs (same id, the session id to resume); the
  window makes the row and says so with `background_promoted`, which lets the helper go, or with an
  error, which unmarks it. A window coming up, or finding the core again, settles what one gone
  meanwhile was opening: a row with a helper's id is made by nothing else. Helpers outlive the core: every 2 s, as
  the core leaves (`stop_all`), and at once on what a core killed next must not undo (a spawn, a
  conversation named, a delete, a question asked, an opening as a chat), the registry goes to `<data>/helpers.json` when it
  has changed, one save at a time so that no older reading is written over a newer one — the
  spec without its token, the session id, the worktree, the turn count, whether a turn ran, the text
  queued behind it, and the agent layer's ties (`agents::Ties`: parent, name, settings, the answers
  owed and counts shown). Of the conversation itself nothing: that is in the CLI's transcript. A
  `send` that queues writes at once, as a core that goes hard writes nothing more. The next core
  reads it before the hub listens (`background::restore`, `agents::tie_back`, fresh tokens): a turn
  that ran then went with the old core and counts as ended, the text queued behind it as the turn
  after. Such a turn, or one `stop_turns` stopped, leaves the helper `cut_short` until its next turn,
  so the owed caller's notice says it did not finish.
  A debt comes back with its helper, ahead of the chat it is owed to: the window registers that chat
  only as it starts its CLI again, seconds later. So the notifier drops a debt for an unknown caller
  only `CALLER_RETURN_MS` (10 min) into the core's life (`owed_stands`), until then telling that
  caller nothing and keeping `reap` off the helper; a delete (`forget`) purges at once as ever. A
  chat not opened by then gets no notice. The text queued behind a stopped turn stays with the helper,
  each piece with its sender (`Queued`, `Entry::undelivered`, saved too), never run: only that sender
  hears of it (what no session sent, its parent). Its notice counts it and the answer file ends on it,
  labelled, word for word, or holds only it if there is no reply (`undelivered_text`); `read` and
  `wait` name it as `undelivered`, and taking it off is part of `hand_over`, so a cancel crossing the
  response puts it back with the debt. A helper deleted or opened as a chat first leaves what is left
  in a file the log names (`undelivered_left`). `luna_kill` still drops the queue: that is the agent's
  own stop. A batch a turn was claimed for is saved as `Entry::in_flight` before the CLI gets it and
  cleared once stdin has it, so a core killed between neither forgets it nor hands back what ran.
  A helper's `budgetTokens` lives there too (`CORE_SETTINGS`), with where it stands, and is a notice
  and nothing else: `budget_news`, on every fifth look of the notifier and only for helpers with one
  not yet told, reads `Context::of` off the transcript or rollout, so both CLIs and visible and background
  helpers alike; the first reading past it stays as `budgetBreach`, as a compaction may take the helper
  back under before its parent can hear, and the line goes to the parent alone. It rides (`riders_go`)
  and is told once it has gone in: typed, or into a turn. Left behind a background parent's turn it is
  `budgetQueued`, the line itself being the mark to look for: gone from the queue a turn took it
  (`budgetTold`), found undelivered after a core went it is taken out of there and told over. A rider's
  gate is not delivery: a draft holds a line past it, as any notice, and a power-off does not wait for that.
  A helper spawned with `resumeOnRestart` (the flag lives in `settings`, so in the ties, and `list`
  echoes it; `register` keeps it over the window's settings, and a chat's way is below) goes on by itself:
  `resume_stopped`, once the hub listens as its MCP config names the port, starts a turn on each that
  is `cut_short`, on a tagged line saying why and carrying on, followed by the undelivered text. Debts
  on it move past the turns counted so far, so only the end of this turn answers them (`resumed`), the
  transcript offset staying, and are marked: the caller gets one notice that it was cut short and
  resumed (`Settled::Resumed`), then the usual one. A turn too short for the first merges them. A
  resume that cannot start counts as an ended turn with its error (`run_turn`), and the notice is the
  plain cut-short one with the text still held. It runs only where nothing of the old turn can overlap
  it (`Entry::resumable`): that core stopped it itself, or its processes were in a job tied to the core
  (`Entry::tied`, saved; off Windows only if `end_last_ties` also finds nothing of the job left). Elsewhere
  (a tie or join that failed, a keeper that could not start, a job not seen out, no record of it) the caller
  gets the plain notice and decides. That stands across cores (`Entry::unsure`, saved, as the next would
  read this one's `cut_short` as its having stopped the turn itself) until the helper's next turn, which
  its caller chose knowing, or a later core's look finds the job over. It takes only the batch it captured, re-checking under the lock that the helper
  is still cut short, as a turn sent meanwhile has gone on already. A helper stopped before any turn named its conversation
  is taken up like any other, so its debt, its lineage and in time its end record in `agents.jsonl`
  survive: what its turn was started on is saved until then (`Entry::opening`) and becomes the first of
  its undelivered text, the notice says it never got started (`never_started`), and its next turn is a
  first turn, which `luna_send` starts, or with `resumeOnRestart` `resume` on that same text. Nothing is
  written before `restore` has read the file, so an empty registry never overwrites it.
  **The chats' ties.** The sessions the window runs have no `helpers.json`, but as much that a core must
  not forget: `<data>/chat-ties.json` (`agents::save_chats`, on `save`'s beat) keeps, for each that was
  spawned, handed a worktree or is owed on, its `Ties`, turn count and transcript, and the sends Luna holds
  (`Held`). `restore_chats` reads it first. A chat is known again from there (`Known::waiting`), its
  lineage, `borrowed`, `auto` and debts with it, until the window starts it and `register` clears the
  mark, so `dropWorktree` still will not touch a worktree it was handed. The count goes on
  (`activity::keep_count`), since baselines go by it. A debt the count had not passed when the core went
  is one whose turn went too (`Owed::cut_short`): that turn counts as ended, as a helper's does, and the
  notice says it did not finish; text going in before that notice does (a held send, say) makes a new
  debt, which carries the word on (`owe_from`). A session given `SESSION_RETURN_MS` (1 min) to come back that has not
  is reported as not started again (a chat not in a pane starts only when opened), a send held for it as
  lost; a caller marked waiting is given `CALLER_RETURN_MS`, as one not known at all. A chat's parent
  has one record, the core's (`Known::parent`, kept here and moved by `pass_up`): `register` takes the
  window's word only where the core has none, not at all for an orphan, and where the two differ says
  `agent://reparent` again, for a new parent the window never heard of before its core went. A row deleted while
  no core ran told nobody: `chat_names`, the first list of rows after, drops what was kept of it. Neither
  CLI goes on with a turn a core killed: Claude Code's `--resume` only does under an environment variable
  Luna does not set, and Codex's own continuation lives in the daemon `--no-daemon` keeps it out of, so
  the window starts it between turns (Claude Code at its composer; Codex there too, saying "Conversation
  interrupted" and writing no end for that turn, so a `task_started` older than this process does not read
  as a turn on now: `scan_rollout`). `save_chats` stops as sessions are stopped (`freeze_chats`: a core
  leaving, `stop_all_sessions`), as what quitting does to them, Codex writing an abort that counts as a turn's
  end, must not be what the next core reads; `register` starts it again.
  **A chat that goes on by itself.** One with `resumeOnRestart` whose turn went with the core (running or
  waiting as last saved, `SavedChat::mid_turn`, or with a debt the count had not passed) is put on
  `Registry::resumes` by `take_up_chats`. A helper usually sits in no pane, and nothing else would start its
  CLI, so the window asks which to start (`agent_resumes`, on every core up) and does as a pane would
  (`startSession`). Starting it there beats a plain notice for such a chat, which would make the flag mean
  nothing for most visible helpers, and the core cannot do it alone: what a chat starts with (its session id,
  its worktree, its settings) is the window's. On `luna-held`'s beat `resume_chats` looks at each
  (`resume_look`): it is typed into once it has stood two looks in a row at an empty prompt (`at_empty_prompt`:
  not by the cursor, which Claude Code hides until it hears it has the focus, as a helper in no pane never
  does), between turns, with no blocking dialog. `resume_chat` takes `TypingInto`, then a person's keys, looks again that it is still
  on the list, untouched and at its prompt, and goes by `type_held`, with `spill` for a long one. A conversation to go on (`Session::resume`)
  gets what a background helper is told (`resume_text`); a CLI that came back afresh, its first turn having
  named nothing, gets the prompt it was opened on (`ChatResume::opening`, from `Opening`, kept in the ties until a first
  turn has ended under a core, as a count kept across one may be only a turn that went with it; a Codex one with
  its note). The text Luna held for it (`Held`) goes with it in order, so `deliver_held` passes it by, and each
  sender is owed as if it had gone in by itself. A debt that went in
  moves past the count, cursor staying, and `resumed` marks the "cut short, resumed automatically" notice, in
  one hold with leaving the list. While it is on the list `notice_line` says nothing of the turn that ended,
  though it still tells of a dialog, `wait` waits (as for held text) and `read` settles nothing. It leaves the
  list for good on: a person's key (`typed_ago_ms`; a draft stays theirs, Codex's and any `type_text` will not
  take), a `send` going in or held (`call_off_resume`, whatever it finds of it under `TypingInto`), its CLI
  gone, no window start within `SESSION_RETURN_MS`, or no prompt within `CALLER_RETURN_MS` (Claude Code may take
  minutes to draw). The caller then gets the plain notice. A later core does not bring it back: a debt marked
  `cut_short` by an earlier core is not a turn that went now.
  **Text a core went in the middle of typing.** A held send, by `deliver_held` or with a resume, is marked on disk
  as it goes in (`Held::going_in`: its first words and the transcript's end) and taken off once typed. Finding one
  so, the next core does not type it again blind (`reconcile_held`): the CLI that was to take it went with that core,
  so what the conversation it came back on holds is what it took. There, it is not typed again and its sender is owed
  the end of a turn cut short; not there (or a new conversation), it goes in like any held text; with no transcript
  to read it is not typed again and the notice says Luna cannot tell (`Owed::unsure`). A debt told "NOT done yet"
  comes back so on a background helper (`Ties::told_pending`), or nothing would ever settle it: on Windows what it had
  running went with the core and no turn is coming, and its notice says the core restarted.
  **A spawn under way.** `spawn` writes a `SpawnTaken` to `chat-ties.json` (`SpawnUnderWay`) once it has settled
  what it asks for, before anything is made: the caller, the request with its prompt and settings, and with
  `worktree` what worktrees there were. `spawn_is` names the session as it registers and before its CLI starts
  (`ensure_*` takes `spawnRequest`), giving it what the window does not carry (`borrowed`, `auto`, the flag) and
  saving at once; the record goes when the call ends, or on a timeout once the window answers. Request ids start
  from the clock, so a page's answer to a core gone since lands on nothing of this core's. Left in the file, it
  means the core went mid-way. `take_up_spawns`, with both registries read and once per spawn (`owed_placed`): one
  that had come to a saved session and no debt has that session owe its caller, as a turn cut short with no turn of
  it ended (and it may go on by itself), as its answer with one. The window may still be part way through one (a
  graceful restart's `ensure_*` waits for the new core): such a one is in `abandoned` too, so a late `agent_spawned`
  owes as ever and clears it. `SESSION_RETURN_MS` in, `settle_spawns` takes what is left, deciding under the lock that
  gives it up (`settled`), past which `spawn_is` refuses a session the window is still starting for it, so its CLI
  never runs and the page drops the row: with a session known by then (a row the window dropped since it failed
  is not), nothing more; with `resumeOnRestart`, `spawn_afresh` asks again, its new record replacing this one in one
  hold, and the notice names the new helper. A window that has not answered that in time is no failure (`still_coming`):
  the caller hears it may yet come up and not to spawn again, and the new record alone is saved (`retried`), so a core
  going then starts one. It hears if that fails too (`spawn_failed_late`), as does any caller of a spawn that timed
  out. Otherwise the caller is told it never completed, with the prompt in a file (`never_completed`). Each says that is why the call itself failed: the HTTP response
  went with the core, as usually did the caller's own CLI, which comes back with that call cut off and nothing
  to go on. A worktree new since the spawn was taken on with no session in it is named as `worktreeLeft` and
  never removed; the Worktrees window can sweep it. Told as a notice is (`spawn_news`, `deliver_notice`), or to
  the log for a caller not back in `CALLER_RETURN_MS`.
  **A first turn a core went with.** A visible helper's first turn cut off leaves two cases, which the same notice
  must not cover: with a conversation, its prompt is in it and "did NOT finish" is true; with none, nothing has what
  it was asked, and "`luna_send` has it carry on" would carry on from nothing. So `spawn_is` keeps every visible
  helper's prompt as it was asked (`Opening`, the note a Codex one is typed with added only then), saved in the ties
  until a first turn has ended under a core; a `floor` of 1 or more says a core went with it. Which case it is goes by
  what is on disk, not by whether the row has heard: the window learns a session id by polling, every few seconds, and a
  core can go first. So before a CLI starts on no prompt `conversation_of` looks: the transcript the last core knew of
  (its path is saved while it is on its first turn), else one begun since it was asked, where it was to work, that holds
  what it was asked and is known to be no other session's. Found, the CLI resumes that, a Codex one where its rollout
  says it works. Not found, it is `Opening::lost`, as a background helper with no `session` is `never_started`, and one
  not back by `SESSION_RETURN_MS` is told of by the same look (`first_turn_lost`). The parent is told it never got
  started, with the prompt and that parent's held text in the file (`undelivered_for`); that text is not typed in
  meanwhile (`deliver_held`), as it would start the helper on the wrong foot. That is all it will ever have to say
  (`lost_whole`), and text it holds will never go in, so `wait` answers at once rather than wait on either (`lost_answer`),
  and `read` collects it: the debt goes with the payload so no notice says it again, and a cancel crossing the response
  puts back both (`take_undelivered`). A helper going on by itself is given its prompt instead, and is none of this unless
  that is called off. Other senders' held text goes in as ever. `lost` outlasts the count, so a turn a person starts in
  the fresh CLI does not swallow what it was asked.
  **A worktree half-made.** A helper's CLI that comes back afresh is `--worktree` again, and would make a second, the
  first unclaimed and unreported as the spawn's own `worktreeLeft` comes only where no helper came of it. Being new since
  the spawn, unclaimed and clean says nothing of whose a worktree is, so what goes on in one goes by a record made for
  it. Claude Code: Luna names the worktree of every helper it spawns (`helper_worktree_name`), recorded with the helper
  (`Registry::worktree_names`, a background one's in its `Spec`), and asks for it as `--worktree=<name>`: as an argument
  of its own a name is read as a flag, and on 2.1.314 `--worktree --dangerously-skip-permissions` turns permissions off
  (`worktree::name_ok`, `worktree_arg`). Given a name, the CLI makes that worktree or goes back into it as it is. A fresh
  start goes back only into that one, and only if it is `as_made`, nobody being in it: on the branch made for it, its HEAD
  the commit git has that branch created at (the oldest line of its reflog, as Luna cannot know it ahead: 2.1.314 starts
  at origin's default branch, not the folder's HEAD, and a core may go before anything could note it), and nothing in it
  that is not in that commit, what git ignores included, but the `.claude/settings.local.json` the CLI copies in. What
  `.worktreeinclude` copies is not told from work. Anything else stays where it is, its parent hears of it as
  `worktreeLeft`, once, with its first notice or `read`/`wait` (`worktrees_left_line`, saved till then), and the helper takes
  the next name (`name_to_go_on`). In a conversation it goes back to its own whatever is in it. Codex takes no name and
  says where it works only with a thread, which is then a conversation to resume; Luna making the worktree itself would
  take it out of Codex's hands. So a Codex helper starting afresh makes a new one, and what is new of Codex's since it
  was spawned is named (`codex_new_since`, `report_codex_worktrees`), in words that say Luna can only tell so far. A CLI
  not back by `SESSION_RETURN_MS` has what it began named (`leave_half_made`), and opened later it makes its own.
  **Deleting a worktree with work in it.** `luna_delete` with `dropWorktree` takes the worktree `remove`
  would (`worktree_to_drop`: not one handed to the session or shared; a Codex worktree off its rollout
  before the first turn's end records it) and, before it touches anything, runs `git status --porcelain`
  and counts the commits that would go for good (`commits_lost`): those HEAD or a `worktree-*` branch
  going with it reach, and no branch that stays and no remote does. That covers a detached HEAD, and a
  throwaway branch left behind by a switch; a branch the session named itself stays, so its commits do
  not count. A checkout removed by hand still has its branch asked about, from the repository, and what
  git cannot tell counts as work. `status` is told not to hide submodules; files git ignores do not stop a
  delete (build output would stop every one), and the refusal and the result say they go with it. It
  refuses with a short list; `force` skips that. Clean at that look, the session is stopped and what it
  left ended (`stop_for_good`), then it looks again: what came up meanwhile keeps the worktree
  (`worktreeLeft`), the session going all the same.
  The session goes before its worktree, since its process holds the files: a removal that still fails is
  not an error (a retry would find no session) but `worktreeLeft` and `worktreeError` in the result. The
  log says which branches were deleted and which kept. The window's Delete and `reap` (which never drops
  a worktree) go by `remove` too: the dialog has warned a person already.
  **One state, and what is asked.** Tool answers carry `state` (`state_of`: working, waiting,
  question, background, idle, starting, exited) instead of a turn word and a busy flag that could
  disagree, and `ask` (`ask_of`): a background helper's held call; for Claude Code the tool its
  hooks name (`PreToolUse` for `AskUserQuestion`, `PermissionRequest` — the transcript gets a
  question's call only once it is answered), else a held tool call in the transcript; for Codex the
  rollout's open question, or an approval its screen shows. Notices quote it, and an async Codex
  question gets a notice of its own while the helper works on. `luna_answer` in a terminal is
  keys: an option is arrows down from the first and Enter, words of its own go in the free-text row
  after the options, several Claude Code questions end on a review that Enter submits; a permission
  is Enter (Codex: `y`) or Esc, a note typed after it; an async Codex question is answered by a
  message. `keys` takes `{enter}`, `{down}`… for any other dialog. `send` into a session on a
  blocking dialog refuses and points to `luna_answer`. A prompt or message past 8,000 characters
  goes to a visible session as a file under `<data>/agents/` (every Claude Code session with Luna's tools or an agent
  above it gets it as an `--add-dir`, as a notice's answer file is there too; Codex's sandboxes read anywhere), swept after a week.
  To test a build without a second Luna as the parent: `LUNA_DEV_TOKEN` (+ `LUNA_DEV_FOLDER`,
  `LUNA_DEV_ACCOUNT`) registers a caller `dev` with that bearer token; its spawns are
  top-level chats. `test-build.ps1 -Run -DevCaller` sets them itself.
- **New-chat dialog.** The model is picked from a drop-down (`ModelSelect`, a native `<select>`) for
  both providers; the model the files (or the CLI) would pick is marked "· default". Codex's list is the
  catalogue in `priority` order, its effort levels carry the catalogue's descriptions (`effort_hints`) as
  tooltips. Codex's approval and sandbox are two radio columns; `auto-review` holds the sandbox at
  workspace-write. The pane title bar shows a `bypass` chip for never + danger-full-access and an
  `auto-review` one for that. Its folder list is the `picked` folders: where a person launched a chat,
  adopted an orphan or browsed to. A folder only an agent ran a helper in holds its row in the sidebar
  and is not offered; folders stored before the flag count as picked if they hold a top-level chat,
  keep a start of their own or are the dialog's last folder.
- **Recommended settings** (Accounts → per-account button). A tick means "the file will have this
  key": applied keys open ticked; unticking one removes the key — only if the file still says exactly the
  recommended value, and parent objects/tables left empty go too; "Tick recommended" adds the
  `defaultOn` set; Apply shows `+N / −M`; "Edit settings.json/config.toml…" opens the hand editor.
  The account row's button shows "k/n" recommended keys present. Claude Code reads a few settings
  (`copyOnSelect`) from `.claude.json` only: such a spec is `global`, its Rec names the file, and apply
  rewrites `.claude.json` only for a key of its own. Rust:
  `apply_recommended(provider, account_path, add, remove)`.
- **The library** (`library.rs`). The user's own instructions and skills, which belong to
  them rather than to any repository (a repository's skills reach everyone who clones it, and
  Luna's MCP instructions every model that connects). Each skill, and each instructions file
  (`instructions/<name>.md`), has a reach in settings.json — `all`, a provider's every account,
  or `provider/name`, the key agents.rs uses too — that account renames and deletions carry
  along; an account a reach stops taking in loses Luna's copy like a skill unticked. An account
  has one instructions file, so where instructions' reaches meet the closest decides (`chosen`):
  named beats a provider beats `all`, and `library.noInstructions` names accounts that get none.
  Setting a reach takes each of its entries from every other instructions
  (`give_instructions`), so two never tie; only instructions the library has compete, and an
  account whose winner changes has Luna's copy replaced like one behind the library — the
  ledger knows a copy by where it lies, not by which instructions fed it. The one
  `instructions.md` of before moves to `instructions/main.md` at the first pass (its reach is
  read as `main`'s); with both there neither is touched and the window says so, and until it
  has moved every instructions unit is `failed`. An account's own file where no instructions
  go is taken in as new instructions named after the account, going into it alone. A folder
  the user names gets nothing written into it: a copy there reaches no worktree (each CLI
  makes one of what git tracks — Claude Code off origin's default branch, Codex off HEAD
  under `CODEX_HOME`) and is one `git add` from the team's repository. Its skills wait per
  account where only a session given them sees them: for Claude Code in
  `<data>/library-folders/<slug>/<account>/.claude/skills/<name>`, passed as `--add-dir`
  (listed under their own names; its `.claude/skills` made before the start, or a skill
  added later never shows); for Codex as a plugin in the account's
  `plugins/cache/luna/<slug>/local/` (manifest written and removed by Luna, outside the
  ledger), off until `-c plugins.<slug>@luna.enabled=true` turns it on, listed as
  `<slug>:<name>`. The slug is the folder's name in `[a-z0-9-]`, no dots: Codex splits a
  `-c` key at each. `session_args` maps a session's folder (a worktree through
  `worktree::root_of` to its project) onto the folders it lies in, at every start: a resume
  or helper turn without them keeps the skills named in its history, unanswerable. Each start
  lays the library out first (`lay_out`), so a skill another program changed on disk reaches
  the next session; the code that starts one calls it before taking any lock of its own (a
  background turn's registry lock would hold every helper's events up for the pass). The
  window's and the core's passes share a lock file beside the ledger, and the window's library
  commands run off its main thread (`command(async)`) to wait on it. A pass goes only by
  settings as the file has them now (`settings::get_checked`): on the defaults of a process
  that could not read them it would take every copy out. A skill
  reaches a folder's sessions on every account but those that have it as a whole (twice
  over, Claude Code would list the second scoped to Luna's folder), or — `foldersOnly` — only
  on the accounts of its reach, and then into none as a whole. A folder can keep a skill from
  its sessions too (`off`, beside its `skills`): one an account has as a whole, or an outer
  folder gives. Nothing moves at layout; at the start the session is told to hide it, the one
  way to keep from it what its account holds — Claude Code by `skillOverrides: {name: "off"}`
  in its `--settings` file (one flag only: `claude_settings_file` puts the hooks and the
  overrides in one, and background helpers get it too), which beats the account's and the
  project's own; Codex by one `-c skills.config=[{name=…, enabled=false}]`, whose entries Codex
  adds to the account's `config.toml` ones selector by selector. Both go by name: a copy an
  outer folder gives is hidden as Claude Code lists it (`luna-<name>` where renamed) or as
  Codex's `<slug>:<name>`, and a repository's skill of the same name goes too. Where a
  session's folders nest, the deepest that names the skill decides (`deciding`), so off in a
  parent can be ticked again in one repository. The window shows, per cell, what a folder's
  sessions would get with no mark (`inherited`: some accounts or all), and a click flips it,
  skipping a mark that would change nothing. A session started outside Luna still has the
  account's copy. A Claude Code copy whose name
  the folder's `.claude/skills` up to the repository root, the account's `skills/` (Luna's
  own copies aside) or its synced skills already have goes by `luna-<name>` (`free_names`,
  `with_name` on its SKILL.md): Claude Code would list ours as `<folder>:<name>`, scoped to
  files under Luna's folder. Such a copy is compared, put back and diffed as the renamed
  skill, and taken into the library under its own name again. The library is the source; a
  destination — an account's instructions file or `skills/<name>`, a folder's copy for an
  account — holds a copy. A pass compares three things per destination: the library's files, what is
  there now, and the ledger's hashes of what Luna wrote there last. Missing → written; there
  and equal to the ledger but behind the library → rewritten; equal to the library with no
  ledger entry → adopted; different from the ledger → `changed` (or `gone`), with no ledger
  entry and different → `taken`: these three are never written, only reported, and the window
  resolves each — take it into the library (which then spreads it), put the library's back,
  or skip that destination (`library.skipped` in settings.json, its ledger entry dropped). The
  side that loses goes to `<data>/library-backups/` first (swept after 30 days). A ledger
  entry no longer wanted is deleted only while what is there still hashes to it; otherwise it
  is left and reported as kept. A skill whose SKILL.md does not do (front matter with this
  `name` and a `description`, the checks both CLIs make) is `failed` — its copies stay, so a
  draft mid-edit takes nothing out — and so is every unit fed by a `skills/` that will not list
  or an instructions file that will not read (held open mid-save): read as empty, they would
  take their copies out. A library folder that is not there wants nothing and touches nothing.
  Paths are compared as the file system tells them apart (`ident`: on Windows either slash and
  any case), so an account renamed in case only keeps its copies and ledger entries. A
  destination inside the library's `skills/` (a library pointed at an account folder) or one
  holding the library is `failed`: it is the source itself, never
  written, adopted or removed. Luna writes through temp files `<file>.<pid>-<n>.luna-tmp`,
  which are no part of a skill or a copy. A link at a destination, or inside it, is never followed or written over.
  Passes run when the window starts, after every change made in the window and on Sync now,
  one at a time under a lock; the window reads the overview (no writes) on every focus. An
  instructions file serves both CLIs; a Codex account's `AGENTS.override.md`, which
  Codex reads instead of `AGENTS.md`, is reported.
- **The hand editor** (`ConfigEditor.tsx`) is CodeMirror 6 — JSON via `@codemirror/lang-json`, TOML via
  `@codemirror/legacy-modes` — with line numbers and colours from theme tokens
  (`--code-key/--code-string/--code-literal`). The core checks problems as you type
  (`check_account_config`, with line/column) and lays the file out on save and on the Format button
  (`format_account_config`: JSON pretty with 2 spaces, TOML via the `taplo` crate, comments and key
  order kept).
- **Theme** — tokens in CSS custom properties (`[data-app][data-theme]`), switching
  system/light/dark without redrawing the terminals (xterm gets the theme via a MutationObserver).
  Shapes are XP (glossy title bars, bevels, task pane, Start button), tones are for long reading:
  neutrals without blue, layers differ by lightness (OKLCH), the terminal is the darkest "well".
  The compiled xp.css pins its controls to light literals, so buttons, fields,
  checkboxes, radios and scrollbars are redrawn in theme.css from the tokens.

## Build

- Locally: `pnpm install`, `pnpm build` (frontend), `pnpm tauri dev` (needs a Rust toolchain).
- Production build — in Docker, see README.
- Test build — `test-build.ps1` (also Docker): `test-build/luna-test.exe` with its own
  identifier (`tauri.test.conf.json`), no updater, everything of its own next to the exe; `-Test` builds
  the Rust tests and runs them on the host. The rules are in AGENTS.md — the one instructions file for
  both CLIs (Claude Code reads it when there is no CLAUDE.md).
- Linux check — `linux-check.ps1` (also Docker): builds the crate for Linux and runs its unit tests
  there, which is the only thing in the repo that compiles and runs the Unix code (`keeper.rs` and
  the like). Its own image and volumes; the checkout goes in read-only.
