# The native window

Release 0.20 moves Luna's window from Tauri + WebView2 + React to GPUI
([ADR-0030](../adr/0030-scope-of-0-20.md)). The native window is built surface
by surface; until it has them all, the WebView window
([frontend.md](frontend.md)) is still in the exe, behind a flag. So far the
native window has its title bar, the theme switch, the core banner, an
empty board, and the desktop services: tray, close to tray, single
instance, notifications, window state and the updater's backend.

## Which window starts

`luna.exe` opens the native window. `luna.exe --webview`, or
`LUNA_WINDOW=webview` in its environment, opens the WebView window as before:
its tray, single instance, notifications, window state, updater and watchdog
with it (lib.rs `run_webview`). The tray's Restart window passes `--webview`
on, so the WebView window restarts as itself. The core, the exit watcher and
the keeper are the same exe with their own flags, and neither window touches
them. `scripts/test-build.ps1 -Run -WebView` starts the test build's WebView
window.

Before either window, lib.rs `run` claims the single instance
(desktop/single.rs): one claim per identifier and data dir, the same for
both kinds, so a native window and a `--webview` one never run side by side
on one data dir. A second launch of either kind hands over to the first,
which comes up (`desktop::bring_up`, set by whichever window runs), and exits
before it has started anything. No claim under `LUNA_DEV_TOKEN`; in the
background a second launch is logged and leaves the window where it is. The
WebView window no longer uses tauri-plugin-single-instance.

The native window does what the WebView window's `setup()` does (native.rs
`run` and `start`): the log's start line with the build stamp (and "(native
window)") and the panic hook first; then, once the window is open
(`Options::on_open`), the event sink, the desktop services
(native/desktop.rs `start`: what a second launch and a notification's click
do, the tray, the session-end hook), the run record and its heartbeat
(lifecycle.rs, which pings whichever event loop it is handed), the core link
(`corelink::start`), the CLI refresh and the settings sync at start.

Closing the window hides it to the tray (below); the tray's Quit ends the
sessions, the core and the window, its Restart window starts a new native
window (`--after <pid>`, no `--webview`) and leaves the core alone, as the
WebView window's do (lifecycle.rs `quit`, `restart`).

A window that cannot open at all (no usable Direct3D device, shaders that do
not compile at startup) has started nothing: `luna_window::run` returns why,
and lib.rs `run` logs it and opens the WebView window instead. On macOS gpui
ends the process as it quits, so there is no fallback there.

## The crate and its host

The window is a crate of its own, `luna-window` (`src-tauri/crates/window/`),
in the workspace `src-tauri/Cargo.toml` makes, on its one lockfile. It does
not depend on `luna_lib`: everything it knows of the app goes through its
host (`host.rs`):

- `Host::call(cmd, args)`: a command answered off the window's thread; the
  window waits for the answer on a task.
- `Host::send(cmd, args)`: keystrokes, resizes and the window's own settings,
  handed over on the window's thread in order, never answered.
- The inbox (`mailbox()`): the host's `Mailbox` sends events, `(name,
  payload)`, and work to run on the window's thread; one task on that thread
  drains the `Inbox`.
- `Host::route_output(session, sink)`: where a session's output goes (below).

The commands, their camelCase arguments and the events are the page's own,
by the same names (`src/ipc/*.ts` is their spec), so the window asks for what
the page asks for. `cmd::ALL` (called), `cmd::SENT` and `event::ALL` list
what the window uses.

The inbox takes only the events in `event::ALL`: `Mailbox::event` drops any
other, and the host's side of `emit` drops them before they are serialized
(`emit::install_for`), so the sessions' output and the page's other events
never queue for the window. An event in `event::LATEST` (`core://state`)
waits in the inbox once at most: a newer payload replaces the one waiting,
but never one with a newer `revision`. The pump (lib.rs `pump`) takes what is
there in batches of 64 at most, each in one update, and after a full batch
lets the thread's other work (input, frames) run before the next: a burst
cannot hold up the window or the heartbeat's ping.

A session's output does not come through the inbox at all. The terminal
routes each session to an `OutputSink` (`Host::route_output`): the host hands
every `pty://output` chunk to it on the thread that read it from the core
(native.rs `Outputs`), the sink feeds the pane's parser on a thread of the
pane's own, and the pane has the window draw once it has parsed. While no
sink is routed, `pty://output` is not even serialized.

luna_lib is the host (`src/native.rs`):

| What the window asks | How it is answered |
| --- | --- |
| a command in `corecmd::COMMANDS` (lane Call) | `corelink::request`, as `core_call` does |
| `send` | `OWN_SENT` for the window's own (`set_window_theme`), else `corelink::core_send` |
| one of the window's own | `OWN`: a name to a function over the Tauri commands' own (`core_state`, `core_restart`) |
| events | `emit::install_for` puts those the window takes (`event::ALL`) into the inbox |
| a session's output | `Outputs`: to the sink routed for its session, if any |

Each call runs on a thread of its own. Tests in native.rs hold the dispatch to
`cmd::ALL` and `cmd::SENT`, the window's own commands to `app_handler!`'s
names where the page has them, the events to what Luna sends, the core's
state as corelink serializes it to the window's DTO (`dto.rs`), the output
routing and the theme's writer.

What the native window keeps between runs is in `<data>/window.json`
(the theme), written by the host; the core's `settings.json` is not the
window's. One thread writes it (native.rs `Writer`): each choice replaces one
not written yet, an earlier one is never written after a later one, and
quitting waits up to 2 s for the last.

## Desktop services

The window reaches the desktop through its host too: `Host` has `Desktop`
(crates/window/src/desktop.rs) for a supertrait, which native/desktop.rs
implements over luna_lib's `desktop/` ([desktop-services.md](desktop-services.md))
and the fake host over `FakeDesktop`:

| `Desktop` | Shape |
| --- | --- |
| `has_tray()` | whether closing hides the window |
| `notify(Notice { title, body, chat_id, sound })` | a toast; a click sends `Request::Show` and `ui://open-chat` `{ chatId }` |
| `read_clipboard()` | `Pending<Clip>`: `Empty`, `Text`, `Files(paths)`, `Image { width, height, png }` |
| `write_clipboard_text(text)` | `Pending<()>` |
| `pick(Pick { kind: Folder or File, title, start, filters }, parent)` | `LocalPending<Option<PathBuf>>`, modal to `parent` (a gpui `Window`) |
| `placement(displays, min)` / `keep_placement(p)` | where the window opens / keeps it (`Placement { bounds, maximized }`, logical pixels) |

`Pending` futures are made off the window's thread; a picker's is awaited on
it (macOS runs dialogs there). Commands beside `cmd::ALL`/`SENT`, in
`desktop::cmd`: `tray_awake` `{ holders }` (sent; the page's own), and
`update_check` (`UpdateCheck { current, offer: { version, notes, pubDate,
coreFingerprint } }`), `update_download` (`{ version }`, with
`update://progress` `{ got, total }` at most every 100 ms) and
`update_install` `{ reason }` (called), which take only what the last check
offered and the last download staged, never a URL or a file from the window.
Events beside `event::ALL`, in `desktop::event`: `ui://open-chat` and
`update://progress`, held in the globals `OpenChat` and `UpdateProgress`
until the board and the status bar show them.

What the host asks of the window itself goes through its mailbox,
`Mailbox::request(Request)`: `Show` (the tray's Open and click, a toast's
click, a second launch), `Hide` (the tray's Quit, at once) and `Quit` (the
end of the tray's Quit or Restart, an update). On Windows the window is
hidden and shown by its own handle (`ShowWindow`), restored if minimized and
brought to the front with `SetForegroundWindow`, using the foreground right
the tray's click, a toast's or a second launch's `AllowSetForegroundWindow`
gave the process (gpui's `activate` would press Alt for it), on a task after
the update the request runs in: `ShowWindow` sends the window its size and
move at once, which gpui cannot take while the app is borrowed. macOS hides
and shows the app. In the background `Show` only shows the window where it is,
off screen and not activated.

`on_window_should_close` hides the window while there is a tray, its place
kept first; without one (it could not be made, or Linux) the window closes
and the app quits. Its place goes to the host half a second after the last
move or resize (`Keeper`), at once as it hides and as the app quits
(`flush_all`, before the host's own quit waits for its writer); the
host writes it to `<data>/window-state.json` through one writer, as the
theme (`Writer`), and quitting waits for it. It opens where the host says,
held to the displays there are now (`window_state::fit`). Neither in the
background.

The tray lives on the window's thread (a `thread_local` in
native/desktop.rs), made once the window is open and dropped as the app
quits; `tray_awake` reaches it there. The session's end is logged with its
reason through `sysmenu::exit_with_session`, hooked on every top-level window
of that thread (the window, gpui's hidden platform window, the tray's),
whichever Windows tells first; under Restart Manager it waits for a model
ask, as the WebView window does. The test build alone subclasses the same
windows for `WM_APP + 0x4C` (wParam 0 Open, 1 Restart window, 2 Quit, 3 asks
whether the tray is up), which runs the tray's own handler, so a script tries
the close-to-tray and Quit paths on a background window without a click.

## Models and views

The state the host reports lives in GPUI entities (`models.rs`): `CoreLink`
(the line to the core, from `core_state` at start and every `core://state`)
and `Shell` (the theme: the choice and the window's appearance). The inbox's
task updates them on the window's thread; a model notifies only when what it
holds changed.

Every state of the core link carries a `revision` (corelink.rs `set`: one
more each time, taken under the link's lock and emitted after it), which the
page ignores. `CoreLink::take` drops a state older than the one it holds:
an event that lost a race to a newer one, or the answer to a `core_state`
asked before an event that came first. Restart sends the revision of the
state the banner shows, and corelink restarts nothing once the link has moved
on from it. Views observe the models they show:

| View | Shows | Observes |
| --- | --- | --- |
| `root.rs` `LunaWindow` | the frame, the font | `Shell`, the window's appearance |
| `title_bar.rs` | Luna's name, the theme button, minimize, maximize, close | `Shell` |
| `banner.rs` | the core banner | `CoreLink`, `Shell` |
| `board.rs` | the board (empty) | `Shell` |

The title bar and the board are cached views (`Entity::cached`): a frame drawn
for the banner reuses them, and they lay out again only when their own size
changes. Frames come on demand (vendor/PATCHES.md, patch 3), so an idle window
draws nothing, and no timer runs in it.

The title bar is the window's own (`appears_transparent`). On Windows its
parts are hit-test areas, so the system moves the window from it, maximizes on
a double-click; elsewhere the drag area starts the move and a
double-click zooms. On macOS the system's traffic lights take the buttons'
place.

The core banner says what the page's CoreBanner.tsx says, in its words, for
`down`, `mismatch` and `elevated`, with Restart the core where a restart
helps (`core_restart`). While the core is being found, started or replaced
(`starting`) it says the window is waiting for it, in a color of its own: that
is never told as a failure. `up` has no banner.

## Theme and font

Two palettes (`theme.rs`): retro, high-contrast, 1 px bevels, black frames,
no gradients, shadows or rounding, status fills saturated with black text on
them. Body text is at 7:1 or better, black on a status fill at 4.5:1 (a test
holds both). The theme is system, light or dark, as on the page, and system
follows the window's appearance. gpui-component's theme is set to match
(colors, square corners, no shadow, no motion, a steady caret), for the
widgets of it the window hosts later.

One font for the whole window, Cascadia Mono, carried in the exe
(`font.rs`, `include_bytes!`) so macOS draws the same letters: the static
Regular and Bold of Microsoft's release 2407.24, under the SIL Open Font
License 1.1 (`assets/fonts/OFL.txt`, and in the third-party notices).

## Background mode

The test build with `LUNA_TEST_WINDOW=background` (`test-build.ps1
-Background`) opens the window past the right edge of every display
(`offscreen.rs`), without focus, having turned on the patched
gpui-pre-windows' switch for unattended windows
(`gpui_windows::set_unattended_windows`; the shots harness does too). That
crate keeps such a window where it is put, shows it there without activating
it or moving it in the z-order, keeps it off the taskbar and has a click not
activate it (vendor/PATCHES.md, patch 2). Without the switch a `focus: false`
window is upstream's. gpui's cap of 30 frames a second for an inactive window is off
(`inactive_frame_interval: None`): a background window never is active.

## Screenshots

`scripts/gpui-shots.ps1` builds `luna-window-shots.exe` (the crate's `shots`
feature) and runs it: every scene of `demo.rs` opened in turn, off screen,
over a fake host, and its frame rendered to a PNG with
`Window::render_to_image`, into `test-build/shots/gpui/<scene>-<theme>.png`.
A shot with nothing drawn fails it. `render_to_image` is behind gpui's test
support, which is why the harness is an exe of its own: the app's exe never
carries it. A new surface adds its scenes to `demo.rs`.

## Tests

The crate's tests run the window on GPUI's test platform over the fake host
(`tests.rs`): what the host says reaches the models and only the views showing
them redraw, a state older than the one shown is dropped, nothing redraws
while nothing happens, the theme button goes round and the host is sent each
choice, system follows the appearance, the banner's wording per state, and
Restart asks the host for the state shown. desktop.rs's tests hold a
notification and its click's request and event, the close to the tray (the
place kept, the window not closed; closed without a tray), the place kept
once after a burst of resizes, and the bounds the window opens with.
`host.rs`'s own tests hold the
inbox to the events the window takes, the core's state to one waiting with
the newest revision, and a batch to its bound. The tests that click
the buttons where they are drawn need debug assertions (gpui records the
selectors only then): the Linux and macOS runs have them, the release-built
Windows tests do not.

## Platforms

Windows is home. The exe's Windows manifest is gpui's, linked in by
gpui-pre's build script, for every mode of the exe (both windows, the core,
the watcher): Common Controls 6, which is all Tauri's own asked for,
per-monitor DPI awareness (v2), `asInvoker`, Windows 10 as supported OS, and
the segment heap. build.rs has tauri-build leave its own out, as two would be
a duplicate resource. macOS is built and tested in CI only
(`.github/workflows/gpui.yml`): gpui-pre-windows and its patches are not in
its graph, and gpui-pre-platform's `font-kit` feature gives it CoreText
(without it no glyph is drawn) and `runtime_shaders` compiles the Metal
shaders at startup. Linux compiles and runs the tests, with no window system
of its own: gpui's X11 and Wayland backends are not turned on.
