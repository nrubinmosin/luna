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
              (the row's and title bar's `draft` tag, the pane's balloon)
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
              accounts each goes into (ReachPicker) and each folder, "Needs a look" with differences (diff.ts — a line LCS);
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
  browsers.rs   which browser a terminal's link opens in: a command line with %1 per
                browser; found on Windows (StartMenuInternet + each Chromium browser's
                Local State profiles, Firefox's profiles.ini, the https UserChoice handler)
                and the user's own; chosen chat → account → folder → default, run with the
                link as an argument of its own and no shell (open_link in lib.rs)
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
                reach winning; `skills/<name>`) — and skills into folders the user names
                (`<folder>/<name>`); a ledger of what Luna wrote where (`<data>/library-state.json`,
                bytes in `library-blobs/` by hash) so only its own copies are touched — see Key decisions
  watchdog.rs   a window whose page stopped responding (WebView2 died) is rebuilt
  coreproc.rs   the core: the same exe with `--core`, no window; one per data dir (core.lock), listens on
                127.0.0.1 with a token from <data>/core/core.json, exits by itself after 10 minutes with no
                window and no sessions (a background helper in a turn is one, in the welcome's count too);
                leaving, it stops every helper's turn with its process tree (`background::stop_all`); on Windows it runs from a copy of the exe <data>/core/<fingerprint>/,
                so the installer can replace luna.exe. pty, hub, agents/mcp, activity and power
                live here; events go to all windows through a per-window queue (a window that falls
                behind is disconnected, the pty does not wait for it)
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
  rollout); delete, `worktree_shared`, `live_cwds` and `agents::caller_worktree` go by it. A background
  helper has no pty to probe: `background::working_dirs` gives the folder it was sent to and the worktree
  it made, between turns too, and `worktree_shared`, `live_cwds` (the orphan sweep) and `account_in_use`
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
  its turns left holds the machine by the same rule, and it is ended with the helper. Hooks print nothing: Claude Code adds the stdout of a hook exiting 0
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
  `luna.log` takes INFO too (log.rs): events only — session start/exit/delete, helper turns, one line per
  MCP call with its steering arguments and outcome but never its text, notices — each session named by id
  and chat name (`agents::label`), so a night can be read back after its sessions are deleted.
  A child born of a turn holds for as long as it lives, with no limit: a `tail -f` left behind and a
  `sleep 64000` before a check due tomorrow look the same from outside, and a PC let sleep or shut
  down on a guess stops work that only a person can start again; no clock runs from the end of a
  turn. The hold itself is logged both ways: `awake hold set: <who>` and `awake hold
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
  The core does not know chats, so a spawn is a round trip: `agent://spawn` to the frontend, which makes the chat
  (`parentId`, not seated in a pane) and `ensure_*` with the prompt as the CLI's positional argument after
  `--` (Claude Code's `--mcp-config` takes any number of files and swallowed the prompt as one more),
  and answers `agent_spawned`; the core waits on a Condvar up to 90 s, and then up to 30 s more — until the
  session takes up the prompt (`started`); if it did not, the answer carries `screen`, and if the screen is still
  empty — `starting`: before its first draw Claude Code syncs the organisation's skills/plugins and
  makes the worktree, and that can take minutes. The prompt meanwhile sits in the CLI's arguments, and anything typed into the pty
  would be lost, so `send` to such a session refuses. `read` and `wait` on timeout also return `starting`. `read` —
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
  none of it: the answer goes to `<data>/agents/<id>-<ms>.md` — the longest message (the report) first,
  then the last, then the rest in order — and the notice gives the count of messages and the path. A
  caller handed a cut quote read the whole answer back anyway, and a long notice reached Claude Code as a
  paste. Typing a notice takes seconds, and the turn it starts may ask again meanwhile: the notifier
  then changes the debt only if it is still the one it announced (same baseline and offset).
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
  a notification with Enter would go out together with the draft. The counter only says there may be
  one: once the keys have settled (1.5 s), `has_draft` reads the composer off the screen
  (`pty::read_composer` — the cursor stands at the caret in both CLIs; Claude Code's `❯` between two rules,
  Codex's `›` down to the first blank row, its dim placeholder left out) and an empty one, or one of
  spaces, zeroes the counter: edits the keys do not tell (Esc Esc, Ctrl+W, a cut selection) no longer hold
  anything until the next Enter. A real draft in Claude Code is put aside for the text (`stash_and_type`):
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
  running; `send` during a turn queues the text for the next, and a turn about to start takes the
  helper's run under the same lock that found it free (`claim`), so text arriving before the process
  is up queues rather than starting a second one. A stop (`kill`, `stop_turns`) that finds a turn
  claimed but with no process yet marks the run `stopped`: `spawn_turn` does not start it, or kills
  what it just started. A turn that cannot start counts as one ended turn with its error, since whoever
  queued it waits on that end; text queued behind it fails with it, as the turn after. On Unix each turn's process leads a process group
  of its own, as a pty's CLI does, so `kill_tree` takes the turn whole. The core stops every turn
  as it leaves (`stop_all`, and no turn starts after), and `stop_all_sessions` before an update stops
  them too (`stop_turns`): a turn still running would keep the old core from being replaced. A Claude Code helper always gets Luna's
  MCP server, if only for `--permission-prompt-tool mcp__luna__luna_permission` (hidden from its
  model by `--disallowedTools`): its permission prompts and `AskUserQuestion` arrive as that call,
  held on a Condvar until `luna_answer` (allow / deny with the input back, or `answers` by question
  text) or the turn's end. Codex's `exec` asks nobody (approval never) and puts a question in its
  reply — or, with `approval: auto-review`, runs `--approve-for-me`, and a reviewer model answers. Helpers are auto-deleted unless spawned with `autoDelete: false`. `send` with `interrupt` takes the
  queue, kills the turn's process tree, waits for its reader to count the turn over, and starts the next
  turn on queue + text (`background::interrupt`); in a terminal it is Esc, up to 3 s for the turn to read as
  over, and the text typed as a prompt of its own. A spawn with `worktree: true` waits up to 20 s for the
  CLI to say where it made it (Claude Code's `init` event, Codex's rollout) and returns `worktree` and
  `branch`, which `list` carries for every helper too. The window gets their list as `agent://background` (the
  count on the parent row) and can hand one to a chat row between turns (`background_promote` →
  `agent://promote`: same id, the session id to resume). Helpers outlive the core: every 2 s, and as
  the core leaves (`stop_all`), the registry goes to `<data>/helpers.json` when it has changed — the
  spec without its token, the session id, the worktree, the turn count, whether a turn ran, and the
  agent layer's ties (`agents::Ties`: parent, name, settings, the answers owed and counts shown).
  Nothing said in the conversation: that is in the CLI's transcript. The next core reads it before
  the hub listens (`background::restore`, `agents::tie_back`, fresh tokens): a turn that ran then
  went with the old core and counts as ended, the text queued behind it as the turn after. Such a
  turn, or one `stop_turns` stopped, leaves the helper `cut_short` until its next turn, so the owed
  caller's notice says it did not finish. One
  whose first turn never named its conversation has nothing to resume and is let go. Nothing is
  written before `restore` has read the file, so an empty registry never overwrites it.
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
  go is taken in as new instructions named after the account, going into it alone. The library is the source; a
  destination — an account's instructions file or `skills/<name>`, a named folder's `<name>`
  — holds a copy. A pass compares three things per destination: the library's files, what is
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
  destination inside the library's `skills/` (a library pointed at an account folder, a folder
  named inside it) or one holding the library is `failed`: it is the source itself, never
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
