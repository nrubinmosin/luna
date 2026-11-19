# The native window

Release 0.20 moves Luna's window from Tauri + WebView2 + React to GPUI
([ADR-0030](../adr/0030-scope-of-0-20.md)). The native window is built surface
by surface; until it has them all, the WebView window
([frontend.md](frontend.md)) is still in the exe, behind a flag. So far the
native window has its title bar, the theme switch, the core banner, an
empty board, the desktop services (tray, close to tray, single instance,
notifications, window state and the updater's backend), and three in-app
windows over the main one: the library, settings and the accounts
([Panels](#panels)).

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
(the theme, the notification sound and the new chat's boxes), written by the
host; the core's `settings.json` is not the window's. One thread writes the
theme and one the preferences (native.rs `Writer`): each choice replaces one
not written yet, an earlier one is never written after a later one, the two
read, change and write the file one at a time, and quitting waits up to 2 s
for the last of each.

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
Each offer has a number (native/desktop.rs `Updates`): a download is kept
only while the offer it was made for is still on offer, and an install takes
only the current offer's; one download runs at a time, let go however it
ends. An install first holds the offer as it is (`Updates::begin_install`)
and claims the one way out (`lifecycle::leave`), which the tray's Quit and
Restart window claim too: while one holds it the others do nothing (a claim
refused leaves the holder's as it was), and until its setup has started a
check, a download or a second install is refused, so a check meanwhile
cannot leave a superseded setup to start. An install that fails gives both
back and takes back the exit reason only with its own token
(`lifecycle::Noted`), so it never clears another way out's.
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
held to the displays there are now (`window_state::fit`), on the display it
was kept on where that is still there (`Placement::on`, the display by gpui's
UUID for it, with the window's place relative to its usable area), else the
one whose usable area holds its center, named to gpui with the bounds
(`WindowOptions::display_id`): gpui takes a window's bounds in the logical
pixels of the display it opens on, and without one takes the primary's and
puts bounds on another display back on the primary. A minimized window is
kept as what it comes back as (`WPF_RESTORETOMAXIMIZED`). Neither in the
background. Unverified on displays of mixed DPI: gpui gives each display's
bounds in its own scale, so the displays' rectangles `fit` compares may
overlap or leave gaps between them there; a window kept on a display still
there is placed by that display alone, which this does not touch.

The tray lives on the window's thread (a `thread_local` in
native/desktop.rs), made once the window is open and dropped as the app
quits; `tray_awake` reaches it there. Nothing sends it yet: the page works
out the awake dot and who holds the PC in its status bar's power chip, from
`power://state` and the chats' names, and the native window's status bar,
a later slice, takes that over. The session's end is logged with its
reason through `sysmenu::exit_with_session(false)`, hooked on every
top-level window of that thread (the window, gpui's hidden platform window,
the tray's), whichever Windows tells first; the hook only records it and
passes the message on, and gpui's hidden platform window then runs the
app's quit, as the tray's Quit does (`on_quit`: the place and the theme
written, the tray removed, a model ask waited for unless the whole session
is ending, events stopped, the power request let go), and exits.

The test build alone (`luna_test`, build.rs: compiled out of every other
build) subclasses the same windows for `WM_APP + 0x4C` (wParam 0 Open,
1 Restart window, 2 Quit, 3 asks whether the tray is up), which runs the
tray's own handler, so a script tries the close-to-tray, Restart and Quit
paths on a background window without a click. It takes only a message whose
lParam is this run's secret: a random number it writes at start to
`test-hook.secret` in the test build's data dir (`test-build/`), a private
file the script reads; a message without it is answered -1 and logged. On a
volume that keeps no ACLs (FAT, exFAT: no `FILE_PERSISTENT_ACLS`), where that
file could not be private, no secret is written and there is no hook, which
the log says.

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
| `panels/` `PanelLayer` | the library, settings or accounts window up, over a scrim | `Shell`; each panel what it shows |

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

## Panels

The library, settings and accounts windows are in-app windows, as the page's
dialogs are: one at a time, on a layer over the whole window (root.rs
`panels`, `panels/mod.rs` `PanelLayer`) that takes every click and hover
while one is up (`occlude`), framed like a window of the period with a title
bar and a close box. Esc (the `LunaPanel` key context's `Close`) closes it,
unless a question, a drop-down list or a rename is open, which goes first,
or the library's editor holds text not saved, which asks once. Every way
the editor's text is replaced or closed goes through that one question
(`LibraryPanel::guard`, `Next`): picking another entry, a rename, Delete...,
Back to Luna's..., Make it the override..., Add folder..., the library's
folder changed or set back to Luna's own (which then leaves nothing picked
and nothing in the editor), restoring a draft, and Esc. The editor takes no
input until its file is read. Nothing opens
them yet but the demo scenes and the test build's driver (below); the title
bar and the status bar will (`panels::open`).

Each is a module of its own and asks the host for what the page's dialog
asks, by the same command and arguments (`panels::cmd`, held by native.rs's
dispatch test as `cmd::ALL` is):

| Module | Shows | Commands |
| --- | --- | --- |
| `library/` | the library's folder and its sync; its instructions, skills, system texts and folders in a list; the one picked: whom it is meant for (`reach_items`, the page's ReachPicker), whose sessions it is for (chats, leads, helpers), what each account and folder gets, and its file in gpui-component's editor (Markdown highlighted by tree-sitter, a steady caret) | `library_*`, `set_library_draft` (sent) |
| `settings/` | General (sound, the accounts folder), Chats (the new chat's boxes), Agents & trust (helpers, trusted folders), CLIs (version, Check, what the automatic rollback turned away; no version is chosen here, ADR-0030), Updates (Luna's, confirmed with its version and notes; the core's restart) | `window_prefs`, `set_window_prefs` (sent), the helpers', `codex_trusted_roots`, `cli_status`, `cli_versions`, `cli_update_now`, the updater's |
| `accounts/` | each account with its CLI, plan and limits; add, rename, delete; the parent account whose settings the others copy | `list_accounts`, `create_account`, `rename_account` and `sync_settings_now` (the core's), `account_identity`, `delete_account_checked`, `account_limits`, `codex_limits`, the settings sync's |

The host answers them in native/panels.rs over the Tauri commands' own
functions, each path argument read through boundary.rs's `AbsPath`,
`AccountPath` or `OrEmpty` (stricter than the Tauri command where that takes
a plain string). These are the native window's alone (`NATIVE_ONLY`):

- `app_version`, Luna's version (the window crate has its own).
- `window_prefs` and `set_window_prefs`, its own preferences (the
  notification sound and how the new-chat dialog's boxes start), which the
  page keeps in its local storage and the native window in `window.json`
  beside its theme, through a writer of its own (`update_settings` keeps the
  writers' read, change and write one at a time).
- `library_write_checked`, the editor's save. `library_read` answers the
  library's folder and the sha256 of what it read (`root`, `revision`), and
  the save writes only into that folder and over that revision, checked
  under the library's lock: a file changed or deleted on disk since, or a
  library folder changed meanwhile, writes nothing and answers `conflict`.
  The editor keeps the text and offers Reload (the file as it is now) and
  Overwrite (the same save, `force`, which lets a changed file go but never
  another library). The page's `library_write` is unbound, as before.
- `library_draft` and `set_library_draft` (sent): the editor's text not
  saved, with what it was read from, kept as it changes in
  `<data>/library-draft.json` (private, one writer, drained as the app
  quits) and forgotten once saved, reloaded or thrown away. A window that
  goes with it unsaved (the tray's Quit or Restart window, which ask nothing
  of the window, or a crash) leaves it there, and the library offers it at
  its next open (Restore, Throw away); restored, its save is held to the
  revision the draft was read from. The last keys before a crash may be
  missing from it.
- `library_rename_skill`: a skill's folder, the `name` of its SKILL.md and
  what the settings say of it (reach, audience, folders, and its left-out
  places). Refused with nothing changed where the skill's folder or anything
  in it is a link, a junction or not a plain file or folder, where the new
  name is taken, and where SKILL.md names the skill but its `name` (quoted,
  folded or literal) cannot be rewritten to read as the new one. The folder
  moves without replacing (a folder of the new name made meanwhile fails the
  move), then SKILL.md, then the settings; a step that fails undoes those
  before it. It answers `committed`, `rolledBack` (as it was) or
  `rollbackFailed` (with what is left where), and the window says which.
- `account_identity` and `delete_account_checked`: Delete... names the
  account's folder by its path and its identity on the drive (volume serial
  and file id, or device and inode), and the yes deletes only while the
  folder is still that one, checked again under the library's lock right
  before it goes; the folder and its provider's folder are held to
  `boundary::account_in` (no link, inside the accounts root). The question
  says a chat still set to the account has nowhere to start: the window has
  no chats until the board comes, so it says so for any; with the board it
  names them, as the page does.

`set_helpers_above_caller` takes `confirmed`: the page's Tauri command asks
the person in a message box of the system's, which a script in the page
could open but not answer; the native window has no page, and asks in its
own question, whose Turn on click is the only way `confirmed` is sent. The
test build's driver has no line that answers it (below), and must not get
one: a driver line is input no person gave.

Sync lays the library out (`library_sync`) and then makes every copy Luna
manages the library's again, as ADR-0030 has it: the copies changed where
they lie are overwritten and the deleted ones put back (`library_put_back`,
over what is there only at the changed ones), the copies no longer wanted
but changed taken out (`library_kept`), each backed up first; a place
holding a file Luna did not write is left alone. It says how many of each it
did, and anything that would not settle, or that changed again meanwhile;
"every copy Luna manages holds what the library says" only when nothing is
left. The core's own passes (at a session's start, after a change) still
only report a changed or deleted copy (library.md).

A CLI's pass comes as `cli://status` (`panels::event`), which the inbox takes
and hands to the settings window while it is up. Long work is said in words
(Syncing..., Checking..., Downloading 0.171.0: 50%), never spun, and a time
(a check's, a limit's reset) is said as the clock time it falls on, so that
nothing counts down: the panels run no timer. The library's editor checks a
SKILL.md or a system text 250 ms after the last key, then nothing more.

The test build's driver (`drive.rs`, compiled in under `luna_test` alone):
`LUNA_TEST_DRIVE=<file>` names a file of lines the window reads every
200 ms and acts on through gpui's own dispatch, keys typed into it
(`keys ctrl-s`, `text ...`) and the panels' own lines (`panel library`,
`select skill review`, `new instructions <name>`, `rename skill <from>
<to>`, `save`, `reload`, `overwrite`, `restore`, `source <folder>|default`,
`sync`, `page clis`, `setting sound off`, `cli-check claude`, `add-account
claude <name>`, `delete <n>`, `yes` for the library's and the accounts'
questions; `panels::drive`), so a smoke run drives the background window
with no global input. No line answers a question of Settings': helpers above
their caller take a person's click, and so does Luna's update.

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
A scene of a panel opens it and runs driver lines in it (`Scene::drive`) on
the fixtures in `panels/demo.rs`.
A shot with nothing drawn fails it. `render_to_image` is behind gpui's test
support, which is why the harness is an exe of its own: the app's exe never
carries it. A new surface adds its scenes to `demo.rs`.

## Tests

The crate's tests run the window on GPUI's test platform over the fake host
(`tests.rs`): what the host says reaches the models and only the views showing
them redraw, a state older than the one shown is dropped, nothing redraws
while nothing happens, the theme button goes round and the host is sent each
choice, system follows the appearance, the banner's wording per state, and
Restart asks the host for the state shown. `panels/tests.rs` holds each
panel's controls to the commands and arguments the page sends (the editor's
load, dirty state, check, save bound to its source and failed save, a
conflict's Reload and Overwrite, the draft kept and offered, every
transition that replaces the editor asking first, a skill rename's three
outcomes, Sync's report; new, rename and delete; reach,
audience and each account's instructions; folders; system texts' compare,
reset and adopt; every settings row; a CLI's Check and its pass;
an update installed only once confirmed; the accounts' add, rename, delete,
limits and settings sync), Esc's order, and an editor left alone that neither
notifies nor asks the host anything. desktop.rs's tests hold a
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
