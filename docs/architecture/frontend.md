# Frontend

Luna's window is native: a GPUI window, the `luna-window` crate, which
`luna.exe` opens by default ([ADR-0030](../adr/0030-scope-of-0-20.md)). The
window owns the chat list and the board; the core owns sessions
([ADR-0001](../adr/0001-session-state-lives-in-the-cli.md)). This page is the
map of the window's parts and where each is described; the detail is in
[native-window.md](native-window.md) (the window itself) and
[desktop-services.md](desktop-services.md) (the tray, notifications, single
instance, pickers, clipboard, window state, updater and installer), and is not
repeated here.

## The crate and its boundary

`src-tauri/crates/window/` is a crate of the workspace `src-tauri/Cargo.toml`
makes. It does not depend on `luna_lib`: everything it knows of the app goes
through the `Host` trait (`host.rs`), which `src-tauri/src/native.rs`
implements and the tests and the shots harness implement over a fake.

| Part | Where | Described in |
| --- | --- | --- |
| `Host`: `call` (answered off the window's thread), `send` (in order, never answered), the inbox, `route_output` | `host.rs`; `native.rs` and `native/` on the app's side | [The crate and its host](native-window.md#the-crate-and-its-host) |
| Event pump: one task drains the `Inbox` in batches of 64, only the events in `event::ALL`, `core://state` kept to the latest | `lib.rs` `pump`, `host.rs` | same |
| Models and views: `CoreLink`, `Shell` and `Workspace` entities; cached views observe the one they show | `models.rs`, `root.rs`, `title_bar.rs`, `banner.rs`, `sidebar.rs`, `board.rs` | [Models and views](native-window.md#models-and-views) |
| Chats, boards, persistence, the 0.19 import, session looks, keys | `state.rs`, `workspace.rs`, `import.rs`, `keys.rs`; `native/state_file.rs`, `native/import.rs` | [Chats, boards and the window's state](native-window.md#chats-boards-and-the-windows-state) |
| Terminal: `TermPane` over `alacritty_terminal`, a parser thread, the core's PTY | `term/` | [The terminal pane](native-window.md#the-terminal-pane) |
| Panels (library, settings, accounts) and dialogs (new chat, quick chat, delete chat, helpers, crash screen) | `panels/`, `library/`, `settings/`, `accounts/`, `dialogs/`; `native/panels.rs`, `native/dialogs.rs` | [Panels](native-window.md#panels), [Dialogs](native-window.md#dialogs) |
| Desktop services: the `Desktop` supertrait of `Host` | `desktop.rs`; `native/desktop.rs` over `src-tauri/src/desktop/` | [Desktop services](native-window.md#desktop-services), [desktop-services.md](desktop-services.md) |
| Theme and font: two palettes, Cascadia Mono carried in the exe | `theme.rs`, `font.rs` | [Theme and font](native-window.md#theme-and-font) |

The window asks the host for what the WebView page asked for, by the same
command and event names and camelCase arguments (`cmd::ALL`, `cmd::SENT` and
`event::ALL` list what it uses). A command the core serves goes through
`corecmd::COMMANDS`, as it did for the page; a few are the native window's
alone (`NATIVE_ONLY`, such as `window_prefs` and `crash_report`). Paths a
command takes are typed in the host (`boundary.rs` `AbsPath`, `AccountPath`).

## What the window keeps

- The chat list is the window's only copy of the person's chats. The window
  sends its whole state as text and the host writes it to
  `<data>/window/chats.json` through one writer, synced, retried until it is
  written, and waited for when Luna quits.
- The theme, the notification sound and the new-chat dialog's boxes are in
  `<data>/window.json`; the window's place is in `<data>/window-state.json`.
  Neither is the core's `settings.json`.
- On the first start the host reads 0.19's WebView store (`luna.chats` and
  the other `luna.*` keys, Windows only) by copying its LevelDB files and
  never opening the store, and the window imports what it can
  ([Chats, boards and the window's state](native-window.md#chats-boards-and-the-windows-state)).

## Testing

The crate's tests run the window on GPUI's test platform over the fake host
(`tests.rs`, `panels/tests.rs`, `dialogs/tests.rs`, `term/tests.rs`,
`board_tests.rs`, `desktop.rs`); the host side has its own in `native.rs` and
its `native/` modules. Rules are held by tests rather than by a framework:
the window's commands and events against `cmd::ALL` and `event::ALL`, the
dispatch against `corecmd` and `app_handler!`. See
[Tests](native-window.md#tests).

- **Shots.** `scripts/gpui-shots.ps1` builds `luna-window-shots.exe` (the
  crate's `shots` feature) and renders every scene of `demo.rs` off screen
  over a fake host, light and dark, into `test-build/shots/gpui/`. A new
  surface adds its scenes there ([Screenshots](native-window.md#screenshots);
  [development](../development.md#native-window-shots)).
- **Test driver.** The test build alone (`luna_test` cfg) has a driver
  (`drive.rs`): `LUNA_TEST_DRIVE=<file>` names a file of lines the window
  reads and acts on through gpui's own dispatch, so a smoke run drives the
  background window (`scripts/test-build.ps1 -Run -Background`) with no
  global input. A second hook, subclassing the window's own handles, runs the
  tray's Open, Restart window and Quit under a per-run secret
  ([Panels](native-window.md#panels), [Desktop services](native-window.md#desktop-services)).

## Retiring the WebView

The React 19 + Vite + zustand window (`src/`, with xterm.js and CodeMirror)
is still in the exe, reachable with `luna.exe --webview` or
`LUNA_WINDOW=webview`, and is the fallback where the native window cannot
open at all (no usable Direct3D device). It is the 0.19 window, kept for now; the
native window is where Luna's surfaces are built.
`src/ipc/` stays the spec of the commands' and events' shapes the native
window follows, and the Tauri capabilities and
[ADR-0015](../adr/0015-the-page-gets-only-what-it-calls.md) still govern what
that page may call. The last stage of the 0.20 work (stage 6) removes Tauri,
React and Node, with `src/`, the `--webview` flag, the fallback and the
checks that exist for them. Until then the repository's frontend gate, tests
and build keep running ([development](../development.md)); the old page's
description is in this file's git history before the native window became the
default.
