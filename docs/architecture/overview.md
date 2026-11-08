# Architecture overview

Luna is a desktop shell (Tauri 2, Rust + React) around the Claude Code and Codex
CLIs. It runs many CLI sessions side by side, each on an account folder of its
own, and lets sessions orchestrate other sessions through an MCP server.

Two principles shape everything else:

- **Session state lives in the CLI.** Luna multiplexes terminals, folders and
  accounts; transcripts, titles, status and worktrees are the CLIs' own and are
  read from their files ([ADR-0001](../adr/0001-session-state-lives-in-the-cli.md)).
- **Two providers, two layers.** `claude/` and `codex/` each describe their CLI
  in its own words; only mechanics are shared
  ([ADR-0005](../adr/0005-two-providers-as-independent-layers.md),
  [providers](providers.md)).

## Stack

| Layer | Choice |
| --- | --- |
| Shell | Tauri 2: Rust core + system WebView (WebView2 on Windows) |
| UI | React 19, TypeScript, Vite; Zustand (+ `persist` in localStorage) for UI state |
| Terminals | xterm.js + FitAddon (+ WebGL addon) |
| Editor | CodeMirror 6, for account settings files and the library |
| pty | `portable-pty`: ConPTY on Windows, openpty on Unix |
| Look | xp.css, redrawn from theme tokens |

## Process model

| Process | What it is | Lives in |
| --- | --- | --- |
| **Window** | `luna.exe`: the Tauri app with the WebView, tray, title bar handling, updater | `lib.rs`, `corelink.rs`, `watchdog.rs`, `sysmenu.rs`, `lifecycle.rs` |
| **Core** | the same exe with `--core`, no window; on Windows started from a copy under `<data>/core/<fingerprint>/` | `coreproc.rs`, `corecmd.rs`, everything that runs sessions |
| **CLI children** | `claude` / `codex` in a pty, or `claude -p` / `codex exec` per turn for background helpers | `pty/`, `background/` |
| **Exit watcher** | `--watch-exit <pid>`: logs how the previous process ended | `lifecycle.rs` |
| **Keeper** (off Windows) | `--keep-ties`: ends a dead core's helper jobs | `keeper/` |

The core owns the pty manager, the **hub** (loopback HTTP for hooks, the guard
and MCP), the agents registry and notifier, activity sampling, power, and
background helpers. One core per data dir (`core.lock`); it listens on
`127.0.0.1` with a token from `<data>/core/core.json` and leaves by itself after
10 minutes with no window and no session
([ADR-0002](../adr/0002-core-process-outlives-the-window.md)).

```mermaid
flowchart LR
  subgraph W["Window process (luna.exe)"]
    P["WebView page<br/>src/ (React)"]
    T["Tauri commands<br/>lib.rs"]
    L["corelink.rs"]
  end
  subgraph C["Core process (luna.exe --core)"]
    CC["corecmd.rs<br/>command dispatch"]
    PTY["pty/<br/>sessions + scrollback"]
    BG["background/<br/>headless helpers"]
    HUB["hub.rs<br/>/hook /guard /mcp"]
    AG["agents/ + mcp.rs<br/>registry, notifier"]
    ACT["activity/ + procs/<br/>busy / waiting / idle"]
    PW["power/"]
  end
  CLI["claude / codex<br/>in a pty"]
  HL["claude -p / codex exec<br/>one process per turn"]
  FS[("CLI files<br/>registry, transcripts,<br/>rollouts, config")]
  DATA[("Luna data dir<br/>paths::data_dir")]

  P -- "invoke (ipc/core.ts call)" --> T
  T --> L
  L -- "loopback TCP, token hello<br/>length-prefixed JSON frames" --> CC
  CC --> PTY
  CC --> AG
  PTY -- "spawn, write, resize" --> CLI
  BG -- "prompt on stdin,<br/>JSON events on stdout" --> HL
  CLI -- "hook POST, MCP POST" --> HUB
  HL -- "MCP POST" --> HUB
  HUB --> AG
  HUB --> ACT
  ACT --> PW
  ACT -- reads --> FS
  AG -- reads --> FS
  CLI -- writes --> FS
  HL -- writes --> FS
  C -- "events (emit.rs),<br/>per-window queue" --> L
  L -- "webview events" --> P
  C -- "settings, logs,<br/>helpers.json, chat-ties.json" --> DATA
```

## Data flow

- **Commands.** The page calls `call()` in `src/ipc/core.ts`, which invokes a
  Tauri command. Commands that belong to the core are forwarded by the window
  (`core_call`, `core_send` for terminal input) and mapped by name onto the
  same functions in `corecmd.rs`, only those in `corecmd::COMMANDS` and each
  on its lane. The page never talks to the core directly, and reaches only
  the commands its capability grants
  ([ADR-0015](../adr/0015-the-page-gets-only-what-it-calls.md)).
- **Events.** Code that emits does not hold an `AppHandle`: everything goes
  through `emit.rs`. In the core each window has its own queue; a window that
  falls behind is disconnected rather than allowed to stall a pty, and goes
  straight back (`corelink.rs` `rejoin`), its panes redrawn from the scrollback.
  Main event families: `pty://output`, `pty://exit`, `power://state`,
  `agent://background`.
- **Status.** Every 5 s `activity/` combines the CLI's turn state (hooks,
  registry, rollout), the processes under the CLI, and recent output into
  busy / waiting / idle per session; `power/` follows that summary
  ([power and lifecycle](power-and-lifecycle.md)).
- **Agents.** A session with Luna's tools calls `POST /mcp` on the hub with its
  bearer token. Every helper it spawns runs in the core, in the background, with
  no part for the window, which only lists them, shows one read-only, and opens
  one as the person's own chat when asked ([agents](agents.md)).

### An agent spawns a helper

```mermaid
sequenceDiagram
  autonumber
  participant A as Agent CLI (caller)
  participant H as hub.rs + mcp.rs
  participant R as agents/ (registry)
  participant B as background/ (core)
  participant S as Helper CLI (headless)

  A->>H: POST /mcp tools/call luna_spawn (Bearer token)
  H->>R: arguments_fit, authorize caller, settle privilege, account, folder
  R->>R: record SpawnTaken in chat-ties.json
  R->>R: register (lineage, token) + spawn_is (task, save the ties)
  R->>B: start: spec, first turn's prompt
  B->>S: one headless run (claude -p / codex exec), prompt on stdin
  R->>R: owe_from: record the caller's debt for the answer
  R-->>H: result: id, settings, sameFolder, worktree
  H-->>A: tool result
  Note over A: caller may end its turn
  S-->>B: turn ends (its event stream)
  R->>A: notifier types "[Luna notice, not typed by the user] ..."
```

Registration and `spawn_is` happen before the process is created, so a helper
can never run unknown to the registry, and a core that dies mid-start leaves a
record to settle ([background helpers](background-helpers.md#a-spawn-under-way)).

### The person opens a helper as a chat

```mermaid
sequenceDiagram
  autonumber
  participant P as Page (BackgroundChip, bridge.ts)
  participant B as background/rows.rs
  participant R as agents/ (handover.rs)
  participant A as Its former parent

  P->>B: background_promote (between turns)
  B->>B: promoting: no turn starts, sends queue, parent's kill/delete refused
  B-->>P: the row's data (same id, session id, no parent)
  P->>P: createChat (top level), setSessionId, seat in a pane
  P->>B: background_promoted (ok)
  B->>R: take_over
  R->>R: one hold: parent gone, debts gone, taken_over recorded
  R->>B: take the record and what was queued for it
  R->>R: one line per session affected, saved in chat-ties.json
  R->>A: notice once A is idle: "The person took over your helper ..."
```

A row the window could not make answers `background_promoted` with the error,
and the helper goes on as its parent's.


## Where state lives

| State | Owner | File |
| --- | --- | --- |
| Conversations, titles, models, context | the CLI | transcripts, rollouts, registries |
| Chat list, board, folders, UI prefs | page | localStorage (`persist`) |
| Luna settings (accounts root, defaults, library reaches) | core and window | `<data>/settings.json`, written under `settings.json.lock` |
| Helpers, their queues and ties | core | `<data>/helpers.json` |
| What agents are owed by the person's chats, held sends, spawns under way, untold lines | core | `<data>/chat-ties.json` |
| Spawn history | core | `<data>/agents.jsonl` |
| Library ledger | core and window | `<data>/library-state.json`, `library-blobs/` |
| Log | both | `<data>/luna.log` |

A settings file that will not read or parse is an error, never the defaults:
the cache keeps what it read last and a write refuses (`settings.rs`).

## Module map

| Area | Doc | Main paths |
| --- | --- | --- |
| Agents and MCP tools | [agents.md](agents.md) | `agents/`, `mcp.rs`, `hub.rs`, `agentlog.rs` |
| Background helpers | [background-helpers.md](background-helpers.md) | `background/`, `agents/persist.rs`, `agents/resume.rs` |
| pty and launch | [pty-and-launch.md](pty-and-launch.md) | `pty/`, `claude/launch.rs`, `codex/launch.rs`, `guard.rs`, `golden.rs` |
| Providers | [providers.md](providers.md) | `claude/`, `codex/`, `cli.rs`, `provider.rs` |
| Accounts and account files | [accounts.md](accounts.md) | `accounts.rs`, `account_settings/`, `checks.rs`, `account_use.rs` |
| Library | [library.md](library.md) | `library/` |
| Worktrees | [worktrees.md](worktrees.md) | `worktree/` |
| Power and lifecycle | [power-and-lifecycle.md](power-and-lifecycle.md) | `activity/`, `procs/`, `keeper/`, `power/`, `asking.rs`, `coreproc.rs`, `lifecycle.rs` |
| Frontend | [frontend.md](frontend.md) | `src/` |

Smaller modules:

| Path | What it does |
| --- | --- |
| `paths.rs` | `data_dir()`: next to the exe when writable, else `%LOCALAPPDATA%\luna`; settings, log, attachments, CLIs and (Windows) the WebView profile; `protect_data_dir` ([ADR-0011](../adr/0011-private-data-dir-beside-the-exe.md)) |
| `settings.rs` | `<data>/settings.json`, written under its lock re-reading the file; the cache checked against mtime, as more than one process writes it |
| `emit.rs` | the one door for events; `pty::manager()` is likewise the process's only `PtyManager`, no Tauri state |
| `secret.rs`, `admit.rs` | minted secrets and constant-time checks; what the loopback listeners let in before authentication |
| `browsers.rs` | which browser a link opens in: a command line with `%1` per browser, found on Windows (StartMenuInternet, each Chromium browser's Local State profiles, Firefox's `profiles.ini`, the https UserChoice handler) plus the user's own; chosen chat → account → folder → default; run by `open_link` (`lib.rs`) with the link as its own argument and no shell |
| `media.rs` | files pasted or dropped into a pane, copied into the data folder |
| `throttle.rs` | the cool-off after a 429 from the usage endpoints, shared by both providers |
| `log.rs`, `agentlog.rs` | `luna.log`; `agents.jsonl` |
| `sysmenu.rs` | Windows' own window menu and frame messages for the undecorated window |
| `watchdog.rs` | rebuilds a window whose WebView died |

Rust paths are relative to `src-tauri/src/`. Security boundaries are in the
[threat model](../security/threat-model.md).
