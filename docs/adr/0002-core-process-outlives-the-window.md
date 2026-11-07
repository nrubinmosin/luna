# ADR-0002: Sessions run in a separate core process that outlives the window

- **Status:** Accepted
- **Date:** 2026-10-17 (`3946cd1`, "Luna starts its core: a second process the
  sessions will live in"; `c6048cf` the same day cut sessions, agents,
  activity and power loose from the window's handle)

## Context

Every pty, hook listener and agent session used to live in the window process.
A WebView2 crash, a hung page or an app update therefore killed every running
turn. Agents made this worse: an orchestrator's `luna_wait` must keep working
while nobody looks at the window, and overnight work must survive a window
restart.

## Decision

Run the sessions in a **core**: the same executable started with `--core`, no
window, one per data dir (`core.lock`), listening on `127.0.0.1` with a token
from `<data>/core/core.json` (`coreproc.rs`). The pty manager, the hub, the
agents and MCP server, activity sampling, power and background helpers all live
there. The window (`corelink.rs`) finds or starts the core of its own build,
forwards the page's core commands (`core_call` / `core_send`), and relays core
events to the WebView. Frames are `u32 len | u8 kind | JSON` (`ipc.rs`).

- On Windows the core runs from a copy, `<data>/core/<fingerprint>/`, so the
  installer can replace `luna.exe` under a running core.
- A window that falls behind is disconnected rather than allowed to stall the
  pty, and rejoins at once, its panes redrawn from the scrollback.
- The core leaves by itself after 10 minutes with no window and no session.
- An update whose Rust side is unchanged (`core_fingerprint`, a hash of
  `src-tauri` written by `build.rs`) restarts only the window.

## Consequences

- A crashed, hung or killed window takes nothing with it; tray "Restart window"
  is safe mid-turn.
- Two processes mean a versioned protocol, a hello/welcome handshake, and a
  policy for a core of another build (replaced at once with no sessions, only
  on a button with sessions).
- A dead core still ends its sessions; this is surfaced (banner, Restart the
  core) and never done behind the user's back. What agents hold is persisted so
  the next core can take it up ([background helpers](../architecture/background-helpers.md)).
- The core token is a full-control credential; see B2 in the
  [threat model](../security/threat-model.md).
