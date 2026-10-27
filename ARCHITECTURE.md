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
              worktrees.store + WorktreesDialog (stale worktrees, the title bar's Worktrees)
    panes/    1/2/3/4 grid, Pane (drop target), Terminal (xterm), panes.store
              (auto-tiling lives there too: TILE_ORDER, board grows/shrinks by ±1 pane);
              peek — a sheet over the dimmed board, the same pane without remounting;
              altPeek.ts — a look at a chat while Alt is held over its row;
              terminals.ts — registry of live xterms that outlive their panes
    new-chat/ creation dialog: folder, account (which also picks the provider), checkboxes
              "Git worktree" and "Luna tools" (start state — from Settings or saved for the
              folder, Folder.start), then the provider's fields (Fields from
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
    settings/ Settings (gear in the status bar and in Accounts): accounts folder, auto-tiling,
              new chats, deletion, Luna/Claude Code/Codex versions and update buttons
    status-bar/ full-width strip of the window: working / waiting (click — to the waiting one), limits of all
              accounts, Accounts, power, settings, help; the update chip — only when there is news
    power/    power field in the status bar and menu: keep-awake, arming "shut down/sleep when everything is done";
              power.store listens to power://state, computes nothing itself
    shortcuts/ the F1 / "?" window in the status bar: all shortcuts and mouse gestures in one list
    agents/   bridge.ts — the frontend half of agents: agent://spawn → createChat with parentId
              and ensure_* with the prompt → agent_spawned; agent://deleted → deleteChat;
              a model asked for without an effort gets that model's default effort;
              agentAccounts.store — which accounts agents may not use (core settings.json)
  ipc/        commands.ts (invoke), events.ts (listen) — the only boundary with Rust

src-tauri/src/
  provider.rs   enum Provider { Claude, Codex } — a tag, nothing more
  accounts.rs   list/create/delete folders in <accounts root>/{anthropic,openai}/<name>;
                a Codex account gets a seed config.toml on creation
  settings.rs   <data>/settings.json: accountsRoot (default Documents/luna-accounts),
                changed in Settings; written under settings.json.lock re-reading the file,
                the cache is checked against mtime — more than one process may write the file
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
                CPU/IO/start time of each, the "busy" rule (judge)
  hub.rs        loopback HTTP (tiny_http) on a random port with a per-launch secret;
                POST /hook/<chat>/<secret> takes Claude Code hooks' stdin;
                POST /mcp with the session's bearer token — MCP for agents; a thread per request
  mcp.rs        the MCP server's JSON-RPC: initialize (with instructions), tools/list, tools/call;
                ten luna_* tools with short descriptions (~2k tokens), offered to sessions given
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
    trust.rs    hasTrustDialogAccepted in .claude.json
    session.rs  registry <config>/sessions/<pid>.json + transcript projects/<cwd>/<sid>.jsonl:
                context, CLI title (custom-title / ai-title), first prompt
  codex/
    cli.rs      Source: GitHub Releases openai/codex, rust-v<ver>, package tarball + SHA256SUMS
    config.rs   config.toml via toml_edit (comments and formatting kept), seed
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
    trust.rs    [projects.'<path>'] trust_level = "trusted" in config.toml
    session.rs  rollout files sessions/YYYY/MM/DD/rollout-<ts>-<uuid>.jsonl: which is ours,
                status, model, context, first prompt, cwd (rollout_cwd — where a --worktree
                session went); thread name from session_index.jsonl
  account_settings.rs  recommended settings (settings.json / config.toml, key by
                key, platform-aware; apply_recommended adds and removes keys), sync from the
                parent account, and hand editing of the file (check_account_config: problems
                with line/column; format_account_config: JSON pretty with 2 spaces, TOML via
                taplo, comments and key order kept; only what parses is written)
  watchdog.rs   a window whose page stopped responding (WebView2 died) is rebuilt
  coreproc.rs   the core: the same exe with `--core`, no window; one per data dir (core.lock), listens on
                127.0.0.1 with a token from <data>/core/core.json, exits by itself after 10 minutes with no
                window and no sessions; on Windows it runs from a copy of the exe <data>/core/<fingerprint>/,
                so the installer can replace luna.exe. pty, hub, agents/mcp, activity and power
                live here; events go to all windows through a per-window queue (a window that falls
                behind is disconnected, the pty does not wait for it)
  corecmd.rs    commands the core answers: the page calls them as before, the window forwards
                (core_call / core_send for input), here they map by name onto the same functions
  corelink.rs   the window side: find the core of its own build or start it, hello, hold the connection
                (its loss = the core died → banner, Restart); a core of another build with no sessions
                is replaced at once, one with sessions — only on a button; core events → webview
  ipc.rs        frames `u32 len | u8 kind | JSON` (REQ/RES/EVENT/SEND), hello/welcome,
                discovery file, token
  lifecycle.rs  why Luna stopped: its own exits with a reason, the `--watch-exit
                <pid>` watcher (exit code: killed / crashed), luna.run with the last sign of life
                for the next launch, event loop hangs and machine sleep; the tray's Restart
                window (a new window started as `--after <pid>`, the core left running)
  paths.rs      data_dir(): next to the exe (portable) or LOCALAPPDATA\luna; the test
                build (LUNA_FLAVOR=test) keeps everything next to its exe in test-build/
  lib.rs        Builder + generate_handler
```

## Key decisions

- **Thin Rust core.** Rust does not know about "chats" — only pty sessions by id and account folders.
  The chat list, layout, settings — persisted frontend state. Smaller IPC surface,
  easier UI evolution.
- **Own CLI binaries.** Luna does not depend on a global `claude`/`codex`: `cli.rs` keeps copies in
  `<data>/claude-cli/versions/<ver>/claude.exe` and `<data>/codex-cli/versions/<ver>/bin/codex.exe`
  (`<data>` is the exe's folder for the portable build, so everything lives on one drive), `current`
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
  is replaced by `--dangerously-bypass-approvals-and-sandbox`), `CODEX_HOME` = the account folder.
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
  `{timestamp,type,payload}`. Which file is ours: for a fresh session — the earliest of those that appeared
  after the spawn whose `session_meta` cwd is the folder, or a Codex worktree pointing back at it; for a
  resumed one — by the uuid in the name; the answer is remembered per chat, so two chats in one folder
  do not share a file. Read on incrementally from where the last look stopped (a turn outgrows
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
  the folder (`worktree::is_worktree_of`, `project_of`); every destructive path is gated on this. The
  orphan sweep (the title bar's Worktrees window, every folder at once) scans each account's `worktrees/` as well as the project's. Removing a Codex worktree also
  removes its bucket dir once empty; only `worktree-*` branches are ever deleted (Codex's checkouts have
  none). Where a session actually works is `pty::ActivityProbe::working_dir()` (Claude registry / Codex
  rollout); delete, `worktree_in_use_by_others`, `live_cwds` and `agents::caller_worktree` go by it.
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
  CPU/IO over the window, or a process born after the last turn began and still alive — that is how
  a background `sleep` is caught, while MCP servers that came up with the session count as furniture) and
  pty output over the last 30 s. Hooks print nothing: Claude Code adds the stdout of a hook exiting 0
  to the context, and silence is free. Claude's registry beats an "idle" from a hook, a hook's
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
- **Power follows busyness** (`power.rs`). Keep-awake — `PowerSetRequest(SystemRequired)`
  with a reason visible in `powercfg /requests`, while busy > 0, released after 60 s of quiet;
  `waiting` does not keep the machine up (nobody is answering), but blocks shutdown (the work is not
  done). Stalled (limit and other API errors) does not block: until the reset nothing
  will move there. Arming with `waiting` > 0 sends a notification with the chat names at once. Arming lasts until Luna exits; while armed the hold is always kept, otherwise the PC would sleep in the
  quiet window. The automaton: busy = 0 and waiting = 0 continuously for quiet_s → a 60 s countdown with
  a notification → action. Shutdown closes sessions via `shut_down` with a 5 s grace (the CLIs'
  exit hooks) and `shutdown /s /t 0`; sleep/hibernate — `SetSuspendState`, sessions live on, arming stays,
  and the clock jump on wake resets the quiet window and the countdown. An armed rule that
  does not fire is explained in `luna.log`: a line `armed, not firing: <chat> <why> [hook …,
  registry …]` when the reason changes (no more than once a minute) and every 10 minutes with no change;
  only WARN/ERROR go to `luna.log`, so everything that decides the night's fate is written at that level.
  A session whose turn has been `Idle` for 2 h (`LEFTOVER_MS`) no longer counts its children at all:
  a `tail -f` a monitor left on a growing log moves forever, and one such session held a whole night.
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
  The core does not know chats, so a spawn is a round trip: `agent://spawn` to the frontend, which makes the chat
  (`parentId`, not seated in a pane) and `ensure_*` with the prompt as the CLI's positional argument after
  `--` (Claude Code's `--mcp-config` takes any number of files and swallowed the prompt as one more),
  and answers `agent_spawned`; the core waits on a Condvar up to 90 s, and then up to 30 s more — until the
  session takes up the prompt (`started`); if it did not, the answer carries `screen`, and if the screen is still
  empty — `starting`: before its first draw Claude Code syncs the organisation's skills/plugins and
  makes the worktree, and that can take minutes. The prompt meanwhile sits in the CLI's arguments, and anything typed into the pty
  would be lost, so `send` to such a session refuses. `read` and `wait` on timeout also return `starting`. `read` —
  from Claude's transcript (user/assistant, without tool_result) or Codex's rollout (`response_item`
  message user/assistant) with a byte cursor (whole lines only); `wait(turn_done)` counts busy→idle transitions in
  `activity.rs` (`turns_ended`; the count grows across respawn/resume too — for Codex only turns after
  the process spawned are taken from the rollout) and returns the last answer, so `read` is not needed.
  After Enter, `send` waits up to 12 s for a turn to begin and also answers `started`/`screen`. `screen` is
  the pty scrollback run through a terminal emulator (`vt100`) at the pty's size: the last
  40 lines of what a person sees in the pane — a popup, an error, a question, which the transcript does
  not have; `read(screen: true)` returns it on request, `wait` — on `timeout` and `waiting`. `list` and
  `spawn` return `settings` — the flags the session actually runs with; for Codex `list`'s `flags` take
  the model catalogue from `models_known` (no CLI run). Rights: only
  the caller's descendants — except `sessions` (all sessions of this Luna launch: account and its folder,
  chat folder, cwd, the CLI's sessionId, path to the transcript/rollout, pid; the frontend sends chat names
  as `chat_names` on every change), `send`, which writes into any of them except its own, and
  `read`/`wait` — also on any except its own (the transcript is on disk anyway, and whoever wrote must
  be able to collect the answer); `kill`/`delete` — descendants only;
  tool depth 1; at most 8 descendants. Children outlive the parent (orphans with ↳; a child in another folder also stands apart, but with the parent's name
  in the tooltip); a helper started without a folder works where its caller does, worktree and all
  (`agents::caller_worktree`, both providers). Deleting a child via `delete` removes its worktree if asked — `remove -f -f`,
  because Claude Code locks its worktree and a killed one does not release the lock; the worktree is where the
  session actually works (`working_dir`), and the project for the removal is derived from the path
  (`worktree::project_of`).
  **A helper does not report on itself** — unlike a subagent, its answer sits in its
  transcript until someone calls `wait`/`read`. So `spawn` and `send` record a debt
  (`owed`: caller, session, `turnsEnded` at the time of the question, and where the transcript ended then),
  and a `wait` that handed over a turn past it clears the debt; a `read` clears it only if the session is idle
  with nothing of its own still running — a read mid-work had seen a turn on the way (a background helper's
  turn that ended on a job, the turn before a queued `send`), and once it cleared the debt the real answer
  went unannounced and the helper was auto-deleted unread. A `send` to a background helper mid-turn owes the
  turn after the running one: the queue starts it the moment that one ends. A notifier thread looks at debts every 2 s and, when the caller is between turns
  on two looks in a row (`Idle`, not starting, the person has not typed into it for 20 s and left no
  unsent draft), types
  one line to it `[Luna notice, not typed by the user] …` with Enter — just like `send`: the helper
  finished a turn, stopped on a prompt (once per stop) or exited. The answer is everything the helper said
  past the debt's transcript offset (`replies_since`), not the transcript's last message: a helper's own
  stale wakeup ("nothing changed") ending a later turn used to stand in for the report. Up to 3000
  characters it is quoted whole; beyond, the longest message and the last (up to 400) are, with the whole
  answer written to `<data>/agents/<id>-<ms>.md` and the path in the notice.
  A turn after which the helper still has processes running waits up to 20 s: they went quiet — "finished",
  they did not — "left work in the background", and the debt moves to the next turn (Claude Code comes back by itself
  when the background task ends). The CLI's own resident helpers (`codex-code-mode-host` and its
  `conhost`) do not count as background work while they do not move (procs.rs, `without_furniture`):
  otherwise every Codex helper was announced "NOT done yet" after a turn. A turn with no answer in the transcript waits up to 5 s for it (Claude
  Code's Stop hook arrives before the last answer lands in the transcript; `wait` waits the same way), and then
  the notification says "no answer" and points to the screen. Luna counts the draft from the person's keystrokes
  in `write_session` (`pty::draft_after`: characters add, Backspace subtracts, Enter,
  Ctrl+C, Ctrl+U reset, Enter right after `\` is a Claude Code line break, not a submit;
  key escape sequences print nothing, text inside a bracketed paste does):
  a notification with Enter would go out together with the draft. Edits the counter does not see (Ctrl+W,
  a cut selection) it does not subtract — then the notification waits for the next Enter, and after a minute
  of waiting Luna says so once with a system notification. `wait(turn_done)` on a helper
  stopped on a prompt also returns (`waiting` with the screen): the turn itself will not end; for 8 s after
  a `send` from the same caller into such a session the prompt counts as answered. A notification is an ordinary turn of the caller; a dev caller without
  a terminal does not get them. While the caller's `wait` on that session is still running (`waits`), that
  `wait` delivers the answer: Claude Code moves an MCP call longer than 120 s to the background, but keeps the request, and the result
  arrives as a task-notification. A notification for such a pair waits, and if the `wait` has still not delivered
  the answer 10 s after it appeared (the client dropped the call), it goes out by itself.
  **What `spawn` says beyond the flags.** An unknown slash command: Claude Code writes `Unknown
  command: /x` and starts no turn — `await_start` sees the line on screen and answers `started:
  false` at once, not after 30 s. `warnings` — screen lines with ⚠ (a model forbidden by the
  organisation, and what replaced it: `settings` do not know that). `sameFolder` — live sessions
  whose cwd (per the CLI's registry) matches the helper's folder, except the caller itself; a helper in
  its own worktree does not get them.
  **Accounts in `list`.** `defaults` — `claude::defaults::flags` / `codex::defaults::flags` for
  the caller's folder: the core does the normalisation and stock values (`source: stock`), the frontend
  only turns them into its labels, so the dialog, spawn and `list` say the same thing. `usage` —
  what the frontend shows in the Accounts window, sent as `account_usage` on every change:
  the core does not go to the limits endpoints itself.
  **autoDelete.** `reap` on every notifier pass: a helper with no debt, no `wait` on it and
  no turn (or one that exited) for 30 min in a row is deleted as via `delete`, without its worktree; to the log —
  `auto-deleted`. Any `read`/`wait`/`send` on it (`shown`) starts the 30 min over, and `list` shows what is left
  (`deletesInMin`). If the person ever typed into it (`typed_ago_ms`), or an agent `kill`ed it, Luna leaves it
  alone. Before any delete of a spawned session `remove` writes a `left` line to `agents.jsonl` — transcript
  path, worktree, last message — which `history` folds into the run.
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
  press only), control characters and escape sequences as is. Claude Code
  reads text and does not need this.
  **The Codex sandbox on Windows.** Without `windows.sandbox` Codex rejects (`approval never`) or
  sends for approval (`on-request`) every command under `read-only` and `workspace-write` — even
  `Get-Content`. So those two modes get `-c windows.sandbox="unelevated"` (a restricted token per
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
  running; `send` during a turn queues the text for the next. A Claude Code helper always gets Luna's
  MCP server, if only for `--permission-prompt-tool mcp__luna__luna_permission` (hidden from its
  model by `--disallowedTools`): its permission prompts and `AskUserQuestion` arrive as that call,
  held on a Condvar until `luna_answer` (allow / deny with the input back, or `answers` by question
  text) or the turn's end. Codex's `exec` asks nobody (approval never) and puts a question in its
  reply. Helpers are auto-deleted unless spawned with `autoDelete: false`. `send` with `interrupt` takes the
  queue, kills the turn's process tree, waits for its reader to count the turn over, and starts the next
  turn on queue + text (`background::interrupt`); in a terminal it is Esc, up to 3 s for the turn to read as
  over, and the text typed as a prompt of its own. A spawn with `worktree: true` waits up to 20 s for the
  CLI to say where it made it (Claude Code's `init` event, Codex's rollout) and returns `worktree` and
  `branch`, which `list` carries for every helper too. The window gets their list as `agent://background` (the
  count on the parent row) and can hand one to a chat row between turns (`background_promote` →
  `agent://promote`: same id, the session id to resume).
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
  goes to a visible session as a file under `<data>/agents/` (a Claude Code helper gets it as an
  `--add-dir`), swept after a week.
  To test a build without a second Luna as the parent: `LUNA_DEV_TOKEN` (+ `LUNA_DEV_FOLDER`,
  `LUNA_DEV_ACCOUNT`) registers a caller `dev` with that bearer token; its spawns are
  top-level chats. `test-build.ps1 -Run -DevCaller` sets them itself.
- **New-chat dialog.** The model is picked from a drop-down (`ModelSelect`, a native `<select>`) for
  both providers; the model the files (or the CLI) would pick is marked "· default". Codex's list is the
  catalogue in `priority` order, its effort levels carry the catalogue's descriptions (`effort_hints`) as
  tooltips. Codex's approval and sandbox are two radio columns; the pane title bar shows a `bypass`
  chip for never + danger-full-access.
- **Recommended settings** (Accounts → per-account button). A tick means "the file will have this
  key": applied keys open ticked; unticking one removes the key — only if the file still says exactly the
  recommended value, and parent objects/tables left empty go too; "Tick recommended" adds the
  `defaultOn` set; Apply shows `+N / −M`; "Edit settings.json/config.toml…" opens the hand editor.
  The account row's button shows "k/n" recommended keys present. Rust:
  `apply_recommended(provider, account_path, add, remove)`.
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
