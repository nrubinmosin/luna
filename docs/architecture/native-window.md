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
| events | `emit::install_for` puts those the window takes (`event::ALL`: `core://state`, `pty://exit`, `pty://input-refused`) into the inbox |
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
| `board.rs` | the board: the terminals it was opened with, side by side | `Shell` |
| `term/` `TermPane` | one session's terminal (below) | `CoreLink`, `Shell`; its parser |

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

## The terminal pane

`term/` is one CLI session's terminal: `TermPane`, a view over a grid and a
parser of alacritty_terminal 0.26 (Apache-2.0; its own PTY and event loop are
not used, the session's PTY is the core's). It was measured in the GPUI spike
before it came here; no Zed crate but gpui is used, read or copied.

**Threads and locks.** The `Term` sits behind alacritty's `FairMutex`, shared
by two threads:

- The pane's parser thread (`parser.rs`) parses the session's output into it,
  at most `PARSE_CHUNK` (4 KiB) per hold of the lock, letting go between
  chunks. Once it has parsed it wakes the pane, once until the pane has taken
  the wake (an atomic flag), through a channel to a task of the pane's on the
  window's thread, never through the inbox.
- The window's thread takes the lock once a frame, in the element's prepaint
  (`element.rs`): to resize the grid to the cells that fit and to copy the
  viewport out with its colors resolved; everything after works on the copy.
  Input takes it briefly too (the mode, the selection, the scroll position).

So a burst costs a frame a wait of one chunk's parse at most, never the
parsing, and an idle session wakes nothing: the cursor is steady and nothing
else moves, so no timer runs and an idle pane draws no frame.

**Bounds.** Nothing the program writes makes the pane grow without limit
(`parser.rs` `Budget`): output held or queued for the parser past 16 MiB, and
replies to its queries waiting for the window's thread past 1 MiB, are
dropped, and once the parser has caught up the pane says so in a line of its
own (`[dropped N KB of output: the pane fell behind]`). An OSC payload is cut
at 64 KiB before the parser sees it (`OscCap`; the terminator still goes
through). A synchronized update (mode 2026) is drawn when it ends, at its
deadline (150 ms, alacritty's, kept as alacritty's own event loop keeps it:
the parser waits for the next chunk no longer than that), or once it has
buffered 256 KiB, so one left open never holds later output or the exit
notice, and its end is never megabytes parsed under one hold of the lock;
the pane's own notices and a backlog draw what live output left buffered
first, and leave nothing open for the live bytes after them. The parser's
messages reach the window's thread 64 at a time, the rest of its work going
between.

**The seam.** The pane hands the host an `OutputSink` (`Host::route_output`)
when it starts its session; luna_lib feeds it each `pty://output` chunk on the
thread that reads the core's line (native.rs `Outputs`), and the sink queues
it for the parser. A sink whose pane is gone says so, and the host lets go of
it.

**The session.** `SessionSpec` names the session, its CLI and how it starts:
`ensure_claude_session` or `ensure_codex_session` with the page's own
arguments; starting one that runs reattaches to it. The pane goes through
`Phase`s:

- `Waiting` while the core is not up: a strip over the grid says the pane
  waits for the core (being found, being replaced, or to run again), in the
  banner's waiting color, never as a failure.
- `Starting` once it is up: the sink holds the output that comes from here,
  and the start is asked. An answer to a start asked before the last is
  dropped; a failure while the core went away goes back to waiting; any other
  failure is said in the pane, as the page says it.
- `Live(generation)`: a fresh session (no backlog) gets the held output and
  its first size. A reattach does what the page's `replayTail` does: the
  backlog is parsed, then wiped (RIS), and the modes the core kept for the
  session put back (`session_modes`); the held output is dropped, since some
  of it was sent before the backlog was taken and is in it already, and the
  modes may be newer than the rest; the CLI is asked to paint again by its
  PTY made a row shorter and, 60 ms later, the size the pane is at, and that
  repaint and everything after is live. Keys and pastes go nowhere until the
  modes are parsed (bracketed paste above all). Queries in a backlog are not
  answered again (the parser's quiet jobs); live ones are, by the pane:
  device attributes and modes (alacritty's replies), the colors from the
  pane's palette (OSC 4/10/11), the text area size, each sent with
  `reply: true` so the core does not count it as a person typing
  (pty/control.rs `write_session`). The cursor position query never reaches
  the pane: the core answers it.
- `Ended`: `pty://exit` with its own spawn's number (one heard before the
  start answered is kept for it), said in the pane as `[session exited]`.
  `pty://input-refused` is said in the pane as the page says it. The models
  hand both events to the panes of their session (`Models::panes`).

When the core goes away the pane waits, and when one is up again it starts
the session there afresh, on a clean grid (the page makes a new terminal).
The `core://state` events in between can be folded into the last one in the
inbox, so `CoreLink` keeps an epoch, as the page's core store does: each `up`
that is not the one taken last (another pid, a newer revision, or a state
other than `up` taken since) begins a new one, and a pane whose start was in
an older epoch starts over, whatever its phase. `pty://start-over` (the core
closed every session for a shutdown of the PC that was then taken back)
starts every pane over too, an ended one included, or once the core is up.

**Input.** Writes and resizes go only while the session runs (io.rs, as the
page's `ptySender`): `write_session` with the text, `resize_session` with the
cells and the grid's size in device pixels (`pixelWidth`, `pixelHeight`: the
cell's exact scaled size rounded, times the cells), sent again whenever either
changes, a scale change included. The core passes the pixels on to the PTY
(TIOCSWINSZ on Unix; ConPTY takes the cells). A size the host refused to hand
over (the core's line full for a moment) is not taken for sent, and goes
again 250 ms later. What is typed while a paste reads the clipboard waits
behind the paste, and pastes keep their order.

- Keys (`keys.rs`): xterm's encoding, from its control-sequence reference: the
  modifier parameter on cursor, editing and function keys, SS3 under DECCKM,
  Ctrl and Alt characters, LNM. Alt with an arrow alone is the page's (Ctrl's
  sequence, so a word left or right; ESC b and ESC f on macOS). Printable keys
  are left to gpui's input handler, so they arrive as text (IME commits, dead
  keys, AltGr). A key the platform wants as text is left to it before any
  named key is looked at: AltGr comes as Ctrl+Alt, and AltGr+Space would be
  ESC NUL. Ctrl with a letter goes by gpui's key name, which on Windows is
  the virtual key's own letter (`MapVirtualKeyW` gives A-Z for VK_A-VK_Z in
  every layout), so Ctrl+C and Ctrl+V work on a Cyrillic layout as the page's
  physical-key match does. The keypad half of DECKPAM is in the encoder, but
  gpui-pre 0.3.12 does not tell keypad keys from the main ones, so nothing
  reaches it.
- The pane's own keys, as bindings in its key context: Ctrl+C copies a
  selection and clears it, and goes to the session as ^C without one;
  Ctrl+Shift+C and Ctrl+Insert copy; Ctrl+V, Ctrl+Shift+V and Shift+Insert
  paste; on macOS Cmd+C and Cmd+V as well. Shift+PageUp and Shift+PageDown
  page the scrollback.
- The IME through gpui's input handler, registered in paint: the composition
  is drawn underlined at the cursor and sent when committed; the candidate
  window goes to the cursor's cell, moved by the composition's width before
  the range; UTF-16 ranges never split a surrogate pair.
- The mouse (`mouse.rs`): modes 1000, 1002 and 1003, SGR (1006), UTF-8
  (1005) or X10. X10's coordinates are raw bytes and `write_session` takes
  text, so they stop at column and row 95, the last a byte of ASCII carries
  (xterm stops at 223); past it, a position is reported there. Each
  button is tracked on its own so overlapping presses release the right one;
  Shift bypasses them. Otherwise drag, double-click and triple-click select
  (simple, word, line). The wheel scrolls the scrollback, reports buttons 64
  and 65 to a program that takes the mouse, and sends arrows in the alternate
  screen with mode 1007.
- Focus in and out (mode 1004) go as writes of their own, so the core tells
  them from keys; the focus follows the window's activation.

**Paste rules** (`paste.rs`), on what the host's clipboard holds
(`Desktop::read_clipboard`, read off the window's thread; it says files
before text before an image, and leaves out a file list it cannot type):

- Files (Explorer's copy): their paths, each quoted for the CLI on this OS
  (double quotes on Windows, POSIX single quotes elsewhere, bare when nothing
  needs quoting), space-separated, as a paste. A path that cannot be typed
  whole (not Unicode, a control character, a `"` on Windows, where POSIX's
  single quotes hold one) is left out, and so is the list it is in.
- Text: line ends as CR, every other control character but tab dropped, C0
  and C1 alike (ESC, ETX and U+009B, CSI's C1 form, among them), with
  bracketed paste (2004) off as well as on, so a paste can neither end itself
  early nor act as keys; with 2004 on, framed by CSI 200~ and 201~
  (alacritty 0.17.0 filters ESC and ETX the same way). No dialog asks first:
  both CLIs take bracketed pastes, as the page did.
- An image, or nothing the window reads (no text, no file): the CLI's own
  image-paste key, which reads the clipboard itself (Claude Code's Alt+V,
  Codex's Ctrl+V), as the page presses it. Whether an image is there is asked
  of the formats on offer (CF_DIB, CF_DIBV5, PNG; on macOS PNG and TIFF);
  none of it is read.
- Files dropped on the pane from outside the window are typed as pasted
  files. The drop is taken by position (`on_drag_move` and a mouse-up
  listener of the element's), not by gpui's `on_drop`: a drop from outside is
  no mouse input to gpui, so after a key press its hover test fails and
  `on_drop` would drop nothing.

A copy goes to the host's clipboard too (`write_clipboard_text`). In the
test build, with `LUNA_TEST_CLIPBOARD` naming a file, the host reads and
writes that file instead (native/desktop.rs `test_clip`), so a smoke run
never touches the clipboard of the person at the machine; an image or
nothing there reads as the text `[test clipboard: image key]`, since the
real key would have the CLI read the person's clipboard.

**Drawing** (`element.rs`): background quads merged per run of equal color;
box drawing and block elements (U+2500-259F) as geometry, from each
character's arms (`boxdraw.rs`), so lines meet and blocks tile at any scale;
each row shaped once with font runs only for bold and italic, every glyph
painted on its own cell in its own color. Truecolor, 256 colors, OSC 4/10/11,
bold, dim, italic, underline (single, double, curly, with its own color),
strikethrough, inverse, hidden, wide characters and combining marks. The
cursor is steady: a block, beam or underline, hollow when the pane is not
focused. Colors come from two palettes of the window's look (`colors.rs`),
light and dark, every colored entry at 5:1 or better on its ground (a test
holds it); the selection is the window's own (white on navy, black on white).
Links are plain text.

**The test build** opens terminals with `--dev-open claude=<account>` or
`--dev-open codex=<account>` (repeated) and `--dev-folder <folder>`, until
the board has its own way (native.rs `dev_terminals`). A smoke run types
through `LUNA_TEST_DRIVE` (`drive.rs`: a file of keys the window dispatches
through gpui as if pressed, read every 200 ms while it is set, a line once
its newline is written), since keys with modifiers cannot be posted to a
window and global input is off limits. These, the test clipboard and the
tray's test hook are compiled into the test build alone: both build scripts
set the `luna_test` cfg for `LUNA_FLAVOR=test`, and a release has none of
their code.

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

The terminal's scenes (`term-*`) feed a pane the demo session's output
through the fake host: colors and attributes, box drawing and blocks, a
selection, an IME composition, an exited session, and one waiting for the
core.

`luna-window-shots --bench SECS OUT.json` is the terminal's burst bench
(`term/bench/`): a pane in an off-screen window fed the GPUI spike's synthetic
bursts (a port of `tb-feed.js`'s generator) through the same seam as a
session's output, every 16 ms from a thread of its own, its frames timed
(render start to paint end), the latency from parse to frame and to the pane
taking the parser's wake, the frame's lock wait and hold, and the grid's
checksum against a Term fed the same bytes headless. It is in the harness only, never in the app's exe.

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

The terminal's tests (`term/tests.rs`) drive a pane through gpui's own
dispatch and the input handler its element registers, over the fake host, its
output through the real sink and parser thread: keys with modifiers and
DECCKM, Ctrl+C both ways, AltGr text, paste with 2004 off and on and in
order with the keys typed while it reads, files pasted and dropped, a bitmap
per CLI, SGR mouse with overlapping buttons, selection and copy, the wheel
and the scrollback, the IME, the size in cells and pixels and a size
refused, a fresh start, a reattach and what came during it, a core replaced
between two states and the core's start-over, the exit and refused input,
the wait for the core, focus reports, color queries and the reply lane,
output past the bound, a synchronized update left open or too long, and an
idle pane drawing nothing. A test waits
for the parse before it lets the window's thread run, and tells gpui's test
scheduler to expect the parser's thread (`allow_parking`). The encoders, box
drawing, colors, paste rules and the bench's checksum have unit tests of
their own.

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
