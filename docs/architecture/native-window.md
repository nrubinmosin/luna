# The native window

Release 0.20 moves Luna's window from Tauri + WebView2 + React to GPUI
([ADR-0030](../adr/0030-scope-of-0-20.md)). The native window is built surface
by surface; until it has them all, the WebView window
([frontend.md](frontend.md)) is still in the exe, behind a flag. So far the
native window has its title bar, the theme switch, the core banner, an
empty board, the desktop services (tray, close to tray, single instance,
notifications, window state and the updater's backend), three in-app
windows over the main one (the library, settings and the accounts,
[Panels](#panels)) and its dialogs (new chat, quick chat, delete chat, the
helpers popover and the crash screen, [Dialogs](#dialogs)).

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
before it has started anything. No claim under `LUNA_DEV_TOKEN` in the test
build (`lifecycle::dev_beside`; any other build ignores it, so two windows
never write one chat list); in the
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
window, then the sessions and the core, its Restart window starts a new
native window (`--after <pid>`, no `--webview`) and leaves the core alone, as
the WebView window's do, each only once the window has written its chat
list (native/desktop.rs `leave_by`; Persistence, below).

The models ask the host nothing until the window has opened (lib.rs
`prepare`, then `begin`): the chat list's read and 0.19's import, the
accounts, the CLIs and the update check all wait for it.
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
(the theme, the notification sound and the new chat's boxes), written by the
host; the core's `settings.json` is not the window's. One thread writes the
theme and one the preferences (native.rs `Writer`): each choice replaces one
not written yet, an earlier one is never written after a later one, the two
read, change and write the file one at a time, and quitting waits for the
chat list's writer first (2 s at most), then for the theme's, the place's,
the preferences' and the draft's together (2 s at most for all four).

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
coreFingerprint, generation } }`), `update_download` `{ generation }`
(`{ version }`, with `update://progress` `{ got, total }` at most every
100 ms) and `update_install` `{ generation, reason, anyway }` (called), which
take only what the last check offered and the last download staged, never a
URL or a file from the window. Each offer has a number (native/desktop.rs
`Updates`), its `generation`, which the window's download and install name:
both are refused once a check has put another offer up, so the release
installed is the one the confirmation showed; a download is kept only while
the offer it was made for is still on offer, and an install takes only the
current offer's; one download runs at a time, let go however it
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
kept first; without one (it could not be made, or Linux) the app quits,
once the chat list is written (Persistence, below). Its place goes to the host half a second after the last
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
quits; `tray_awake` reaches it there. The status bar's model sends it, working out
the awake dot and who holds the PC from `power://state` and the chats' names
as the page's power chip did (below). The session's end goes through
`sysmenu::exit_with_session(false)`, hooked on every top-level window of
that thread (the window, gpui's hidden platform window, the tray's),
whichever Windows tells first. The hook first drains the host's writers
itself (native.rs `drain_at_session_end`: the chat list, up to 2 s, then the
theme, the place, the preferences and the draft together, up to 2 s more),
on that thread and needing nothing of gpui, whose own handler cannot run the
app's quit while its app is borrowed (it then only posts WM_QUIT, which
Windows may not wait for). Only then is the run marked as ended on purpose,
with how the drain went; a process killed before that is reported at the
next start as a run that did not close itself. The hook then passes the
message on, and gpui's hidden platform window runs the app's quit, as the
tray's Quit does (`on_quit`: the tray removed, the chat list tried once
more, a model ask waited for unless the whole session is ending, events
stopped, the power request let go), and exits. What the window's state
still held back for the gap between writes (`GAP`, half a second; a change
of the chats themselves is never held) is written only if that quit runs.

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
(the line to the core, from `core_state` at start and every `core://state`),
`Shell` (the theme: the choice and the window's appearance), `Workspace`
(the chats and the boards, below) and `Vitals` (the status bar's, below). The inbox's
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
| `root.rs` `LunaWindow` | the frame, the font, the import's notice; the window's keys | `Shell`, `Workspace`, the window's appearance |
| `title_bar.rs` | Luna's name, boards I-V, layouts 1-4, Clear board, Auto-tile, the Menu (Library, Accounts: panels below), the theme button, minimize, maximize, close | `Shell`; `Workspace`'s board, layout and auto-tile only |
| `banner.rs` | the core banner | `CoreLink`, `Shell` |
| `sidebar.rs` | the board's chats by folder, or the collapsed strip | `Workspace`, `Shell` |
| `board.rs` | the panes in their layout, their title bars and menus, the peek | `Workspace`, `Shell` |
| `status/bar.rs` `StatusBar` | counts, accounts with meters, the update, CLI and power chips, Settings, and the panels over them (below) | `Vitals`, `Shell` |
| `status/orphan.rs` `OrphanLine` | the orphan line | `Vitals` (its count and asking only), `Shell` |
| `term/` `TermPane` | one session's terminal (below) | `CoreLink`, `Shell`; its parser |
| `panels/` `PanelLayer` | the library, settings or accounts window up, over a scrim | `Shell`; each panel what it shows |

The title bar, the sidebar, the board, the status bar and each terminal are cached views
(`Entity::cached`): a frame drawn for the banner reuses them, and they lay
out again only when their own size changes. Frames come on demand (vendor/PATCHES.md, patch 3), so an idle window
draws nothing, and no timer runs in it.

The title bar is the window's own (`appears_transparent`). On Windows its
parts are hit-test areas, so the system moves the window from it, maximizes on
a double-click; elsewhere the drag area starts the move and a
double-click zooms. On macOS the system's traffic lights take the buttons'
place. Below 980 px (`title_bar::COMPACT_BELOW`) the bar is compact: the
captions "Board" and "Panes" go, Auto-tile shows its state by its face
alone and the theme button names the choice alone ("System", "Light",
"Dark"), so the whole bar (about 930 px full, 810 compact) keeps every button
in reach at the 900 px minimum width. The status bar does the same for its
CLI chips ("CC 31%", "CX failed") where every chip in words would leave the
accounts less than 160 px (`ACCOUNTS_MIN`).

The core banner says what the page's CoreBanner.tsx says, in its words, for
`down`, `mismatch` and `elevated`, with Restart the core where a restart
helps (`core_restart`). While the core is being found, started or replaced
(`starting`) it says the window is waiting for it, in a color of its own: that
is never told as a failure. `up` has no banner.

## Chats, boards and the window's state

`Workspace` (`workspace.rs`) holds the window's state (`state.rs`
`WindowState`) and what is only this run's beside it. The state is plain
data with the rules that change it, ported from the page's stores
(chats.store.ts, panes.store.ts) less what 0.20 cut:

- **Chats and folders.** A folder is a path chats were started from, kept
  when it empties (the new-chat dialog offers its `picked` ones). A chat has
  its CLI, account, board (0-4), session id, worktree flag and path, tools,
  model, effort and permission or approval and sandbox, its name and
  where the name came from (`NameSource`: user, a CLI rename, the CLI's title,
  the opening prompt; `may_rename` keeps a name once settled), and its last
  status (kept, so a window started again does not notify of a chat already
  waiting). Context, drafts and held messages are this run's (`Live`).
- **Boards.** Five, fixed. Each has its layout (1-4) and the panes of each of
  its four layouts, each remembered apart, so 4 -> 1 -> 4 brings back what was
  there. Layout 3 is pane 1 left, 2 and 3 stacked right; layout 4 is two by
  two, row by row. Auto-tile fills and empties in `tile_order` (1 -> 2 opens
  the right half, 2 -> 3 splits the right column, 3 -> 4 the left one), so a
  growing board moves no chat but the one it must. Splits are even.
- **Peek** (`Peek`, not kept): a pane of the board held up, or a chat with no
  pane. `show_chat` is a row's click (one pane: the pane shows it; more: a
  chat on the board is held up from its pane, one off it comes up over it,
  the same row again puts it back), `open_chat` a notification's (never puts
  back), `to_board` seats the peeked chat (`auto_place`, or else the pane last
  worked in). Switching board or layout drops the peek and the active pane.
- `put`, `swap` and `unseat` are the pane menu's moves; `evict` takes a
  deleted chat off every layout of every board, auto-tile closing each gap.

**Persistence.** The chat list is the only copy of the person's chats, so
every way it could be lost is answered (native/state_file.rs). The window
writes no file: it sends its whole state as text with a number
(`save_window_state` `{ text, seq }`, sent, so the host has the states in
order), and the host's one writer (native.rs `Writer`) writes the latest to
`<data>/window/chats.json` and lets an older one go. The file is the chat
list's alone, and nothing else writes it: the window's place is
`<data>/window-state.json` (desktop services, above), and neither file is
ever read as the other. No released Luna kept the list anywhere else. The
write is synced (`secret::write_private_synced`: the new file flushed
before the rename, the folder after it where the OS can), and the `window`
folder's parent, the data dir, is synced once per run before the first write
is acknowledged, whoever made the folder (Unix: the single instance's lock
makes it at start, and a folder found there was never synced by the writer;
Windows has no such call, and NTFS journals the folder's creation), so a power
cut leaves the old list or the new one whole. Each attempt is told back to the window
(`window://state-saved` `{ seq, error }`); one that fails is logged and
tried again (500 ms, doubling to 10 s) until it is written, counts as done
only once written, and the window shows a line while the newest failure is
not yet followed by a success. The wait is the writer's, not the state's:
a newer state sent meanwhile waits it out too and is the one tried next, so
a burst of changes on a failing disk is no burst of writes, and only a
write that succeeds starts the wait over. A send the host would not take is
sent again after `SEND_RETRY` (1 s).

**Quitting** waits for the list. Every way out but the Windows session's
end (the tray's Quit, Restart, an update's install, the window closed where
there is no tray) asks the workspace (`workspace::quit`): it sends what
waits for the gap, then calls `flush_window_state`, which wakes the writer
out of its wait, tries the latest state at once and answers `{ seq }`, the
newest state written (5 s at most), or why not. The app quits only if that
`seq` is the newest the window has sent when it handles the answer: a change
made after the host finished (the exit hook's own write could fail, with no
one left to retry) is sent and flushed again, five times at most, then said
like a failed write. Where it
was not written (or the host would not even take the last change), the
window comes up (from the tray if need be) with a line saying so, **Retry**
(asks again) and **Quit anyway**, which quits and lets go of what was not
saved, logged as the person's choice. A quit under way is not asked twice.
The tray's Quit and Restart window claim the way out
(`lifecycle::leave`) and ask the window to quit (native/desktop.rs
`leave_by`), and nothing else of them happens before the window's app quits
on a written list (`on_quit`, native.rs): only then is the run marked as
ended, the core shut down (Quit) or the new window started (Restart window,
`lifecycle::start_window_after`). A quit the window turns down is told to
the host (`quit_refused`): the claim is let go of (the core still up, another
way out not refused) and the way out set aside, so that the notice's Retry
or Quit anyway still goes the way it was asked for. The session's end
(sign-out, shutdown) does not ask, as Windows will not wait: the host's hook
drains its writers itself (desktop services, above), and the app's quit
tries the list once more (2 s) and logs an error if it was not written.
`Saver` writes as the page's throttled storage did: a change after a quiet
spell at once, then at most one write per 500 ms (`GAP`), the same text
never twice; a change of the chats themselves (one added or deleted, its
session id, account, board, folder or worktree path: `critical_key`) at
once; what waits is written as the app quits (`workspace::flush`, before
the host's writers drain). Nor is anything made that the list read after
would replace: no chat is created, no account renamed or deleted, until the
host's state has come (`Workspace::list_wait`, asked as a chat's making
starts and again as it is listed; the new-chat dialog says "Loading
chats..." meanwhile). Nothing is written before the host's state has
come.

Reading it back (`window_state`, `{ saved, kept, aside, readError, old,
oldError }`):

- A file that is there and cannot be read (shared, denied, a folder in its
  place) is `readError`: the window imports nothing, writes nothing, and
  shows a line with **Retry**, which asks again; so does the next start.
- One read at a time: the window asks one (`read_state`) and asks no other
  until it is answered, and takes the answer only where the list was not
  changed since it was asked; one that comes after a change is let go of,
  logged, and Retry is there to ask again. The host serializes reads too
  (`window_state`), so an import's copy never runs beside another.
- A file that is no JSON, or no list by its shape, is renamed
  `chats.unreadable-<secs>-<hex>.json` beside it (`aside`), never deleted;
  the list starts empty, written at once, and a line names the file.
- A list is read piece by piece (`WindowState::read`): a chat or a folder
  that does not read (a CLI this build does not know, no path) is left out
  and counted, a status or a name's source it does not know reads as none,
  the boards are padded or cut to five, a setting that does not read takes
  its default; a file of another version is read the same way. A status or
  a name's source let go of counts as lost (`Lost::unknown`), as does a chat
  listed twice, kept once (`Lost::twice`): what the file said goes once the
  list is written. Where anything was left out (`Lost`), the host first
  copies the file as it was
  to `chats.kept-<secs>-<hex>.json` (synced; one with the same bytes kept
  before is reused), or, failing that, answers `readError`; the window uses
  what it read and names the copy in a line until OK.

**Import.** With no list, or one whose import failed and was left empty
(`import_retryable`: `imported.ok` false and no chats), `window_state`
answers with what 0.19's WebView window kept as well. That store is
`<profile>/EBWebView/Default/Local Storage/leveldb` (Windows only), and the
host reads it from these profiles alone (`import::sources`, `read_old`):

- The data dir's `webview` (paths.rs `webview_dir`), where every 0.6.32 to
  0.19 kept it, whatever the data dir was (an install folder Luna could not
  write to has `LOCALAPPDATA\luna` for one, and that is where its store is).
  Where this store is there at all, it is the one: with a list, with none,
  or with one that does not read. A portable exe in a folder it cannot
  write to has `LOCALAPPDATA\luna` for its data dir (paths.rs `data_dir`)
  and so reads the installed Luna's store: it shares that data dir, and
  everything else in it, with the installed Luna on purpose.
- Only where it is surely not there (the profile, or a folder on the way
  down, not found), and only for an installed Luna (`import::installed`:
  the exe's folder is the one the installer puts it in for this user,
  `HKCU\Software\luna\Luna`'s or else `LOCALAPPDATA\Luna`, with
  `uninstall.exe` beside it), `LOCALAPPDATA\rm.luna` (Tauri's
  `app_local_data_dir`, 0.6.31), then `LOCALAPPDATA\ai.sprouter.luna` (up to
  0.6.30): the first that holds a `luna.chats`, else the first that is
  there. These hold a list only for an install last started on 0.6.31 or
  before: `adopt_webview_profile` moved them into the data dir when the
  WebView window of 0.6.32 to 0.19 started. They are per user, not per
  install, so a portable copy or another install reads none of them: it
  would otherwise import the installed Luna's list. The test build reads
  only its own (`rm.luna.test`, `ai.sprouter.luna.test`, the install of
  `Luna Test`), so in practice none.

A folder from the profile down to `leveldb` that cannot be looked at (any
error but not found), is not a folder, or is a link or junction (any
reparse point, looked at without following it, which could lead to
another profile, another user's) is an error the import says, never taken
for absent, so no older store is read in its place. Once a legacy store has
been read, an older one's failure is logged and let go of, with no line:
it cannot be the list last used. Which store it read and took is logged, by
path alone. None is moved, changed or removed: the host copies the files of
the store (all but `LOCK`; plain files only, no link) into a private folder
of its own, `<data>/window/import-<pid>-<hex>`, made with
`secret::private_dir`, and removes it after; one a run before left behind
(and the `<data>/import-0.19` of earlier builds) goes at the run's first
read, before anything is copied, and never while an import reads
(`import::clean_up`). The store itself is never opened: opening a LevelDB
recovers and writes, and a 0.19 window may still have it open and writing.
A copy is taken only if the store's files (name, size, time) were the same
before and after it, and one that changed, whose listing or copy failed (a
file the old window holds a moment), or that does not open or read, is
taken again, three times in all; the last failure is the one said. Only the
app's own origins are read (`http://tauri.localhost`,
`https://tauri.localhost`, `tauri://localhost`), never a dev server's, and
of those the first in that order that holds a `luna.chats`: Tauri 2 serves
the app on Windows from `http://tauri.localhost`, which every Luna on
Windows wrote to (none set `useHttpsScheme`). A fuller list of another
origin is an older one, not taken over it; where the lists' sizes
(`luna_window::chat_list_size`) differ, each is logged. Where the first's
list does not read, the import says why rather than take another's: the
origin is chosen from the keys alone (the first that holds a `luna.chats`
key at all), and only then is its value decoded, so a value that does not
decode (an encoding byte that is neither of the two below) is an error
for the import to say, never a reason to take another origin's list. With
no list, the first with any `luna.*` key. Chromium's layout is a key `_<origin>\0<key>` whose
key and value each start with an encoding byte (0 UTF-16LE, decoded lossily
so a cut surrogate costs one character, not the key; 1 Latin-1).
`import.rs` makes the state of `luna.chats` (zustand's `{ state, version }`,
version 2), `luna.panes`, `luna.sidebar`, `luna.newchat`, `luna.delete` and
`luna.notify`; groups past V go to board V (their panes are not kept), and
cut fields (colors, handoff marks, account moves, Codex's hooks without
asking, splits) are let go. A row that is no chat or has no CLI or account,
a folder with no path (its chats with it) and a chat listed twice are left
out and counted (`Found::lost`, the tolerant reader's `Lost`): before the
window gets the list, the host copies `luna.chats` as it was to
`chats.kept-<secs>-<hex>.json` (`luna_window::import_lost`, `oldKept`), or,
failing that, answers `oldError` instead; the window imports what read and
names the copy in the same line as for its own file. A list whose state
holds no `folders` array is a failure, not an empty list. A list of a
version before 2 (`import::OLDEST`, written from 0.3.0 on; a page of 0.3.0
or later migrated an older list to an empty one, so only an install last run
before 0.3.0 has one, and v0.1.0's says none, 0) is not imported
as an empty one either: the window logs its version and says in a line, OK
only, that the chat list from an older Luna could not be imported. Nothing
is written for it until OK, which records that the line was seen
(`imported.seen`, with `imported.older`) and writes nothing else of it: no
list is imported. Once the list is written (OK, or a change), `imported.older`
keeps the import from being tried again (`import_retryable`). An import
that reads is written at once, so the next start imports nothing. An import that fails (a store that cannot be looked at, does not
open, read or hold still, or is reached through a link; a list that does not
read) starts empty, logs why and shows a line
with **Retry** and OK, and is not written, so Retry and every later start try
again while the list stays empty; a change the person makes is written, and
still leaves it retryable while it has no chats. No store in the places
above (0.19 never ran there), or only ones with no `luna.chats` (a profile
that kept only its theme), is nothing to import and says nothing.

**Sessions.** No timer runs while no chat can be looked at (the list not
come, the core not up, no chat on an account Luna lists): what makes one
lookable looks at once, and each round arms the next. Hidden to the tray
(`desktop::Shown`) the looks come as before for `HIDDEN_AFTER` (5 minutes),
then every `HIDDEN_EVERY` (a minute), as the hidden page's timers came
under Chromium's intensive throttling (the page itself never paused them),
and one comes at once as the window is shown again. Every `EVERY` (4 s), and once the core is up, the chats with a
known account go to `sessions_meta` in one call, a round at a time: status
(`Status::of`, the page's `mapStatus`), the CLI's title or else the opening
prompt, context, draft and held, session id, a worktree path the CLI made,
Codex's model. A chat with no live session rests; one with a session id and
a name it may still lose has its saved title looked up once (`saved_title`).
A chat turning to waiting is a desktop notification (`Host::notify`, the
page's words and sound setting), once per wait (`announced`: a chat leaves
it when a look says it is not waiting or its session exits; one kept as
waiting is announced already at start); so is the core's `ui://notify`. A
click on one sends `ui://open-chat`, which `open_chat` takes (on the chat's
board). An answer is fenced (`Fence`): it is dropped whole if the core's
epoch changed since it was asked (the core replaced), and for a chat whose
session exited since; a call that fails, or does not answer in 20 s
(`POLL_TIMEOUT`), changes nothing and the next tick asks again. The host
gives the request up at the same time (`corelink::request_within`, its
pending entry removed), so a connection that stopped answering piles up no
threads behind the window's looks. A failed `list_accounts` is asked again
after 1 s, doubling to 30 s, until a list comes (one ask at a time), and
again when the core comes up with none: the looks need the accounts. An
accounts folder that is not there is no accounts; one that is there and
cannot be read is a failure, not an empty list (`accounts::list_in`). `pty://exit`
is taken by the pane whose spawn it is (`generation`), which rests the chat
and marks it ended; one of an older spawn is dropped; with no pane, the chat
rests. While the core is not up, every row and pane title shows `no core`
(`Status::NoCore`, shown, never kept) instead of the last status, and
Ctrl+Shift+W finds nothing. Nothing redraws when a round brings nothing new.

**Names for the core.** The core knows sessions by id only; the workspace
tells it every chat's name and board (`chat_names` `{ names, groups }`,
boards from 0, as the page's bridge sent them), which `luna_session_list`
names and groups the person's chats by: once the list is read, at every
change that alters a name or a board (a change that alters neither sends
nothing), and again to each new core. One the core did not take is sent at
the next change, or to the next core.

**Terminals.** The board makes a chat's `TermPane` the first time the chat
is shown, from `Workspace::spec_of` (the page's `startSession`: account path
from `list_accounts`, Claude Code's worktree by name), and
keeps it while it is shown and for the three chats shown last (`WARM`). A
chat shown again after its session ended gets a new one, which starts the
session anew and takes the keyboard (the board reconciles its terminals
before it moves the keyboard). Every start and restart of a terminal asks
`spec_of` again, so a session id, account, folder or flag that came or
changed since the terminal was made is the one resumed: a terminal made
before the first look would otherwise start a new conversation in a new
core. In a new core (`NewEpoch`) only the shown terminals start again: the
warm ones are let go (`Board::drop_hidden`; `TermPane::shown`), as the
page's were. A chat whose account is not listed, or before the accounts
have come, says why instead of a terminal; a core being found or replaced is
the terminal's own wait.

**Peek and Esc** (`LunaWindow::escape`,
`Board::peek`). A peek that opens takes the keyboard to its frame
(`Board::peek_focus`), not its terminal, and Esc then closes it at once, for
a chat or a pane held up. A click in its terminal, or a key typed on the
frame (`TermPane::forward_key`: the terminal takes the keyboard and the key,
text included), gives the terminal the keyboard, and from then on Esc goes
to the CLI (to interrupt it, or to turn a prompt down): the window's Esc
propagates whenever a terminal has the keyboard. Shift+Esc closes the peek
whatever has it. The peek's strip says which holds now.

**Keys** (`keys.rs`) are bound in the root's key context, an ancestor of
every terminal's, so they work wherever the keyboard is; the terminal's own
(Ctrl+C, Ctrl+V) are deeper. A key that means nothing at the moment calls
`propagate` and goes on to the terminal: Ctrl+digit past the layout or the
list, Esc with no menu, rename or peek to close.

Ctrl+1-4 and Ctrl+Shift+1-4 go by the physical key, as the page's `e.code`
did. gpui on Windows names a key by the character the layout gives it
(vendor/gpui-pre-windows events.rs `parse_normal_key`, `MapVirtualKeyW`),
and with Shift by its shifted character, the Shift dropped: the digit row's
first key is `ctrl-1` on US QWERTY and `ctrl-&` on French AZERTY, Ctrl+Shift
on it `ctrl-!` (which the binding `ctrl-shift-1` never matched) and
`ctrl-1` respectively, and the keypad's 1 is `ctrl-1`. So on Windows the
window intercepts keystrokes before the bindings (`watch_digit_row`,
`cx.intercept_keystrokes`, for focus inside the main window only): a Ctrl
keystroke is that digit, with Shift a layout, where VK_1-VK_4 or
VK_NUMPAD1-4 is down (`GetKeyState`, in step with the window's messages)
and the keystroke is named as the layout names that key in the state Shift
is in (`key_names`: `MapVirtualKeyExW` and `ToUnicodeEx` on the window
thread's layout, the keyboard's state left alone, with Caps Lock on and off;
a keypad digit is its digit). gpui's keystroke carries no key code (the
modifiers, the name and the typed character, which Ctrl drops), and a digit
merely held is not the key pressed: Ctrl+/ typed while 1 is down is the
terminal's. Shift decides which name counts: on Canadian French the row's 3
is `/` with Shift and the keypad's Divide is `/` without, so Ctrl+Divide
with 3 held is the terminal's (0x1f) and Shift+3 is the layout's. What no
event can settle is Ctrl+Shift+Divide with 3 and Shift both down: it reads
as Ctrl+Shift+3. One
that means nothing goes on to the bindings and the terminal. No gpui patch. The bindings stay for macOS, Linux
and the tests. Seen in the code of gpui-pre-windows 0.3.12 and Win32's
documentation, not yet on a machine with an AZERTY layout.

**Seams** (`workspace::Seams`) are where the next slices plug in: `new_chat`
(the New chat button, Ctrl+N), `quick_chat` (Ctrl+Shift+N) and
`delete_chat` (a row's Delete, with the delete dialog's "stop or keep its
helpers"), which then call `Workspace::add_chat` and `remove_chat`. Unset,
they log and do nothing. The tray's awake dot (`tray_awake`) needs the
power state, not the chats: it is the status bar's.

## The status bar and the orphan line

`status/` is the page's `StatusBar`, `PowerChip`, update and CLI chips and
`OrphanSessions`, on GPUI: a model, `Vitals` (`vitals.rs`, in `Models`), and
two views over it, `StatusBar` (`bar.rs`, cached at 24 px, the last row of the
window) and `OrphanLine` (`orphan.rs`, a row above it that is nothing at all
while there is no orphan). Their fills are the palette's status fills with
black text; a count with nothing in it is a plain sunken reading.

| Surface | From the host | Does |
| --- | --- | --- |
| Counts | the chats' status in `Workspace` (`shown_status`: none while the core is not up) | Waiting calls `Workspace::next_waiting`, as Ctrl+Shift+W does; Background opens the helpers popover for every chat (`Seams::helpers`, plugged by `DialogLayer`) |
| Accounts | `list_accounts` (the workspace's list), `account_limits` / `codex_limits` per account | the dot, provider tag, name and per window its key (5H, W, a model's initial), meter, percent and time to reset |
| Luna update | `update_check` once at start, `update://progress`, `update_download`, `update_install { generation, reason, anyway, core }`, `stop_all_sessions`, `update_called_off` | the chip and its confirmation, the bar's own panel, which the Settings' Update... opens too |
| CLIs | `cli_status` at start and `cli://status` | a chip per provider while its update pass downloads (loud while it is the first) or has failed (a click runs `cli_update_now`) |
| Power | `power_state` each time the core comes up, `power://state`, `set_keep_awake`, `arm_power_off`, `disarm_power_off`; `tray_awake` goes out | the chip, its panel, the countdown's Cancel (Retry after one that failed); "power: no core" while the core is out of reach |
| Settings | none | a button that opens the settings panel (`Seams::settings`, plugged by `PanelLayer`) |
| Orphans | `orphan_sessions { known }` every 30 s while the core is up (the timer runs only then) and the chat list has been read, on a change of the chat list and when the core comes up, one look at a time and given up after 20 s; none while the window is hidden to the tray, one as it is shown again; `kill_session` | "N orphan sessions: kill all", which asks once before it kills, and kills only the sessions it asked about |

The commands are in `status::cmd` and the events in `status::event`
(`power://state`, taken as a `LATEST` event, and `cli://status`), beside the
desktop's: `Mailbox` takes them (`event::wants`), and luna_lib's dispatch test
holds `status::cmd::CALLED` to what native.rs answers. `account_limits`,
`codex_limits`, `cli_status` and `cli_update_now` are the window process's own
Tauri commands, so `native.rs` lists them in `OWN` (the first two over
`tauri::async_runtime::block_on`, their path checked as an account's); the
rest go to the core.

**Nothing ticks.** The bar redraws when what it shows changed (the model
notifies only then), and the one clock is the power countdown's: while
`countdownEndsAtMs` is set, a task wakes once a second and redraws; it is
dropped as the countdown ends or is canceled. Everything else is an event or
a slow poll that draws only if its answer differs: the accounts on the
page's own cadence (`usage::Cadence`: a minute between rounds, a rung of
30, 60, 120, 300, 600, 900 seconds after a round where an account has no
numbers, three minutes after a 429, never less than the endpoint's word; one
account at a time, 3 s apart; the first round when the accounts are listed),
the orphans every 30 s, the update once at start (a failed startup check is
quiet; a check the person asks for, by the chip of a failure, is not). The
"resets in" words are as old as the last answer: they are brought up to date
with each round, and redrawn only when a label changed; a round whose only
news is the time the numbers were taken redraws nothing (that time still goes
to the core). A test holds the bar to no redraw through an hour of this.

**The power rule is the core's, and only the newest word counts.** The core
numbers each state it sends (`PowerState::revision`, from 1 in each core
run); the model keeps the number of the state it shows and takes a state, an
event or an answer, only from the core up now and only if its number is not
lower: a reply that comes late cannot put back a countdown a newer state
canceled. An answer is taken only if it was asked in the same core epoch,
and so is the failure of a Cancel. An event carries no epoch of its own, and
its numbers begin again with a new core, so the reader of the core's line
(`corelink::hold`) stamps each `power://state` with the link revision of
the `up` that began its connection (`status::event::stamp`, key `core`): the
model drops an event stamped with another core's, and the inbox's one
waiting copy (`event::LATEST`) is replaced by the later by (core, revision),
so an old core's state 50 neither outlives nor outranks the new core's 1. While
the core is not up the rule is not known: the chip says `power: no core`,
no countdown is shown or ticks, and the panel does not open; the core is
asked again (`power_state`) as it comes back. A Cancel (`disarm_power_off`)
that fails is kept (`Vitals::cancel_failed`): the chip says `cancel failed`
and its button is Retry, until a state without a rule comes.

**What Kill all kills.** Its first click keeps the sessions shown (each id
with its pid) under a number of its own; a look that finds another set, Keep,
or a new core closes the question. The second click looks again
(`orphan_sessions` with the chats known now, one look at a time: a look
under way is let go of, its answer not taken, and none starts beside the
recheck, the schedule's waiting until it is done) and kills only if the
question it was asked for, by its number, still stands and the same
sessions are there, each by `kill_session`, leaving out one whose id a chat
took meanwhile; otherwise it shows what it found and asks again, and a look
that fails kills nothing. The host's request is given up after the same 20
seconds as the window's wait (`corelink::request_within`), so a core that
stopped answering leaves no thread waiting. `kill_session` takes an id
alone and cannot tell a chat's session from an orphan, so the window makes
that check; what is left is the moment between the look and the kill.

**The accounts' room.** The window's width less the other fields' (worked
out from their text at Cascadia Mono's cell) is what the accounts get; they
give detail up a step at a time, together (`usage::Fit`): first the
countdowns, then the meters, then all but each account's fullest window; what
still does not fit is cut off at the field's edge.

**The usage goes to the core too.** `luna_account_list` gives agents each
account's usage as the person sees it: the model sends it (`account_usage`,
the page's bridge) when it changes and again for a new core, since the core
asks no usage endpoint of its own.

**The update.** One state (`updates.rs` `Update`): idle, available,
downloading (`got`, `total`), stopping, installing, unsaved, failed. The yes
of the confirmation (`Vitals::install`, not taken while a check is under
way, whose answer could put another offer up) downloads the installer; where
the release's `coreFingerprint` is not the running core's every session is
stopped (`stop_all_sessions`, shown as "stopping sessions"; the chip opens
the panel, whose Call off drops the wait, `Vitals::call_off_stop`, and a
stop not answered in `STOP_LIMIT`, 60 s, calls the update off by itself,
the chip saying why). From before the stop the workspace is frozen
(`Workspace::freeze`): the person's changes are refused (a click on a pane,
which would change the chat written as the active one, included) and a
notice says why, and so is a notice's Retry, which would read the list
again over it. So are the dialogs' (`Workspace::unfrozen`, asked before
anything goes to the host): a new or quick chat, a chat's delete, a helper
opened as a chat and an account's rename (`rename_account_on_host`) are
refused with the reason, and the workspace's own `add_chat`, `list_chat`,
`place_chat`, `remove_chat` and the remembered folders do nothing while
frozen. A rename or a helper's opening let through before the freeze
(`Workspace::admit`) still makes its change, the chats following the
account or the row taken off again where the core did not settle the
opening (`unlist_promoted`). Once the sessions are stopped, or kept for the
same core, the list's last write is made (`Workspace::save_for_install`):
it waits for that work, for a round of looks under way and its saved
titles, no new round starting meanwhile (`settle`, at most `SETTLE_LIMIT`,
25 s, past which the install is refused as unsaved); the window's place
still waiting out its half second is handed to the host and later moves
are held back (`desktop::seal_places`); the preferences, the theme and the
library's draft go as they change. The list
is then written by the quit's rule (`flush_window_state`: the `seq` the
host says it wrote must be the latest sent, or it is flushed again), and
from the host's yes nothing more is sent (`sealed`): what the core's looks
change after it (a status, a session id, a worktree, a title) shows at
once but is kept in memory only, so no write comes after the host's
drain. From the stop (or the choice to keep the sessions) until the
installer starts, the update holds the core (`Vitals::core_held`): the
banner's Restart is not shown, the Settings' is disabled and
`Models::restart_core` asks nothing; a call off or a failure lets it go.
The installer starts only under the core the last stop took: the one the
stop's answer names (`{ stopped, core }`, `desktop::CoreId`: pid, start
and build), since the stop can wait for a core still starting that the
window has not heard of yet, or the one up when the sessions are kept.
The core is told by its process, not by the window's epoch, so the same
core found again after it let the window go (corelink.rs `rejoin`) is still
it; another one, a restart or a crash and a new start meanwhile, calls the
update off with "Luna's core changed while the update was getting ready;
check again" (`desktop::CORE_CHANGED`), and the next click checks again.
The window names that core in `update_install { generation, reason,
anyway, core }` once the list is written, whatever its own view of the
link says by then (which lags: `starting` still, say, for the core the
stop waited for), and leaves the check to the host; with no core to name
it refuses itself.
Its host holds the core from there (corelink.rs `hold_for_install`): no
`core_restart` is admitted, the page's or the window's, and no way back
(`rejoin`) to another core is taken, until the setup has started or the
install is off; a way back refused so waits for the install to be off,
the link `starting` meanwhile. It drains every writer in one pass (the
chat list, the theme, the place, the preferences, the draft: native.rs
`flush_for_install`), or refuses with `unsaved: `; then the setup is
checked again (`updater::install`), and last, right before
`ShellExecuteExW`, the core it is connected to must be `core` and no
restart admitted before the hold may be under way (`CoreHold::check`),
else it refuses with `CORE_CHANGED` too: the window's view of the link
lags, and a restart it asked during the download can still be running on
the host. That check keeps the hold's lock until the setup has started
(`corelink::Checked`): no connection is made current and no restart is
admitted in between.
The host's side of the stop (corelink.rs `stop_all_sessions`) answers with
the core of the connection the stop went down; where that connection ends
before the answer, so that whether the sessions were stopped is unknown,
it asks again once the window is back with the same core, within
`STOP_LIMIT`. Neither that ask nor the first goes out once the update was
called off since the stop began (`update_called_off`): the call off and
each ask's admission take one lock, so a stop is queued before the call
off or not at all. Luna ends as the install starts.
A list the window could not get written, or an answer beginning `unsaved: `
(what the host holds could not be written first, desktop-services.md), opens
the bar's panel with the reason and three buttons, the list still frozen:
Retry and Install anyway (`anyway: true`) go through it all again, the core
up now looked at anew for whether the sessions must stop; Cancel, when the
sessions were stopped for it, calls the update off (`update_called_off`) so
that the core thaws the chats, starts again each pane the stop asked to end
(`TermPane::start_over`, resumed or reattached as after a core restart),
whether its exit has come or not, since the stop can return before the exits
do, and the exit of the old session that comes late is told from the new
one's by its generation (`TermPane::on_exit`), thaws the list and puts the release back on offer. Any other
failure does the same and leaves an "update failed" chip whose click offers
the install again, except a refusal because another offer is up now
(`desktop::OFFER_CHANGED`), after which the click checks again. Thawed, the
list is written as memory has it, and a place held back is kept. The tests
hold each of these paths, the download and the stop included.

**Seams** (`Workspace::seams`, plugged by the slices that own the thing
behind them): `settings` (the Settings button; `PanelLayer` opens the settings
panel), `helpers` (the Background count; `DialogLayer` opens the helpers
popover for every chat). Where a seam is not plugged, what calls for it is
logged and does nothing. Luna's update has no seam: every install goes
through the bar's update panel and `Vitals::install`, so each one freezes
the list, stops the sessions for another core and has the write confirmed
before the installer starts. The Settings' Updates page shows the bar's
`Update` as it stands, its Check is `Vitals::check`, and its Update... (or
Choose... for an install refused as unsaved) closes the settings panel and
opens the bar's panel (`Vitals::open_update`): one confirmation.

**The power panel** (`bar.rs`): the keep-awake switch, the actions offered
(restart on Windows only), how long quiet (1, 2, 5, 10, 30 minutes), whether
agents start nothing new meanwhile (drain), Arm and Disarm, who holds the PC
and who the armed rule waits on. The chip reads "awake · N" while held,
"<action> when done" while armed (with "· draining" and "· N waiting for
you"), and "<action> in Ns" with a Cancel while counting down. The tray's
awake dot (`tray_awake { holders }`) is told when who-holds-the-PC changes,
by the chats' names, and the plain icon once at start.

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
thread that reads the core's line (native.rs `Outputs`), with the chunk's
`offset` where the core sends one (its text's place in all the session has
sent, [pty and launch](pty-and-launch.md#scrollback-and-terminal-state)),
and the sink queues it for the parser. A sink whose pane is gone says so,
and the host lets go of it.

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
- `Live(generation)`: a fresh session (no backlog, ending at offset 0) gets
  the held output and its first size. A reattach does what the page's
  `replayTail` does: the backlog is parsed, then wiped (RIS), and the
  session's modes put back; the CLI is asked to paint again by its PTY made a
  row shorter and, 60 ms later, the size the pane is at. From a core that
  numbers its output (`replay_at`), the answer carries the offset the
  backlog ends at (`backlogEnd`) and the modes as of there (`modes`), and
  the output from that offset on goes after the modes exactly
  (`Parser::attach_at`): a chunk held before the answer or come after it is
  dropped where the backlog has it, cut where it crosses the backlog's end,
  and parsed live after that, so nothing shows twice, nothing is missed and
  a query in it is answered once. An escape sequence the backlog ends inside
  of (`unfinished_escape`) goes after the modes too, so its rest finds it
  whole. A chunk that starts past where the backlog ended means output was
  missed (the pane's bound, a chunk of an older start): the sink holds
  everything from there (`UiMsg::Gap`) and the pane starts over. From an
  older core (`replay`) the modes are asked for apart (`session_modes`) and
  the held output is dropped, since some of it is in the backlog already and
  the modes may be newer than the rest; the repaint draws the screen whole.
  While it reattaches the pane says so (`Reattaching to the session...`) and
  takes no input, not even to encode a key, until the modes are parsed
  (bracketed paste above all); replies to the program's queries go
  meanwhile. Each completion (the modes, the parse, the bound) acts only for
  the start, the core's epoch and the phase it was made in, so a late one
  unlocks nothing; one that does not come within `REATTACH_WAIT` (15 s), the
  modes' error, or a gap starts the session over rather than take input
  with modes the pane does not know. Queries in a backlog are not answered
  again (the parser's quiet jobs); live ones are, by the pane: device
  attributes and modes (alacritty's replies), the colors from the pane's
  palette (OSC 4/10/11), the text area size, each sent with `reply: true` so
  the core does not count it as a person typing (pty/control.rs
  `write_session`, which takes it so only for one terminal answer whole).
  The cursor position query never reaches the pane: the core answers it.
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
again 250 ms later. Input (keys, pastes, the IME, the mouse, focus) has a
gate of its own besides (`Io::set_typing`): shut while a reattach waits for
the modes, when replies already go. What is typed while a paste reads the
clipboard waits behind the paste, keys as keys, encoded by the modes the
program has as they go; pastes keep their order. At most 256 entries wait
(`QUEUE_MAX`): past that, input is dropped and said once
(`[input dropped: too much typed while a paste waits]`). A clipboard that
does not answer within 5 s (`CLIPBOARD_WAIT`) loses its paste, said in the
pane (`[paste dropped: the clipboard did not answer in 5 s]`), and what
waited behind it goes; its answer, if it comes later, is dropped.

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
- Text: line ends as CR, every other control character but tab dropped, C0,
  DEL and C1 alike (ESC, ETX, DEL, which would act as Backspace, and U+009B,
  CSI's C1 form, among them), with
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

**The test build**'s smoke runs drive the window through
`LUNA_TEST_DRIVE` (`drive.rs`: a file of lines the window acts on, read every
200 ms while it is set, a line once its newline is written): keys dispatched
through gpui as if pressed, since keys with modifiers cannot be posted to a
window and global input is off limits, and the board's moves (`row`, `put`,
`swap`, `unseat`, `toboard`) and `dump`, which writes the board's state into
Luna's log. These, the test clipboard and the
tray's test hook are compiled into the test build alone: both build scripts
set the `luna_test` cfg for `LUNA_FLAVOR=test`, and a release has none of
their code.

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
input until its file is read. The title bar's Menu lists Library and Accounts and opens
one with `panels::open`, which the demo scenes and the test build's driver
(below) call too; Settings is the status bar's button, as the page has it.
They are in a menu, not buttons, for the bar's width: with the Menu it
needed about 930 px (measured in the shots at 1000), a little over the
smallest window's 900, which the status bar taking the theme button and
Settings is to bring back under. The layer
starts below the title bar, so the window's own buttons and the Menu stay
in reach, and the Menu switches panels.

Each is a module of its own and asks the host for what the page's dialog
asks, by the same command and arguments (`panels::cmd`, held by native.rs's
dispatch test as `cmd::ALL` is):

| Module | Shows | Commands |
| --- | --- | --- |
| `library/` | the library's folder and its sync; its instructions, skills, system texts and folders in a list; the one picked: whom it is meant for (`reach_items`, the page's ReachPicker), whose sessions it is for (chats, leads, helpers), what each account and folder gets, and its file in gpui-component's editor (Markdown highlighted by tree-sitter, a steady caret) | `library_*`, `set_library_draft` (sent) |
| `settings/` | General (sound, the accounts folder), Chats (the new chat's boxes), Agents & trust (helpers, trusted folders), CLIs (version, Check, Roll back, what the automatic rollback turned away; no other version is chosen here, ADR-0030), Updates (Luna's, as the status bar has it, Update... opening the bar's confirmation; the core's restart) | `window_prefs`, `set_window_prefs` (sent), the helpers', `codex_trusted_roots`, `cli_status`, `cli_versions`, `cli_update_now`, `cli_roll_back`, the updater's |
| `accounts/` | each account with its CLI, plan and limits; add, rename, delete, sign in (below); the parent account whose settings the others copy | `list_accounts`, `create_account`, `rename_account` and `sync_settings_now` (the core's), `account_identity`, `delete_account_checked`, `account_limits`, `codex_limits`, the settings sync's, `ensure_*_session` and `kill_session` for the login |

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
  library's folder by path and by its identity on the drive, and the sha256
  of what it read (`root`, `rootId`, `revision`), the file read through the
  root's handle held once (no link on the way), so the text and the identity
  are of one folder whatever the path does meanwhile, and the save writes only
  into that folder, through a handle held to that identity, and over that
  revision, checked under the library's lock and again right before the
  file is replaced (docs/architecture/library.md): a file changed, made or
  deleted on disk since, or a library folder changed meanwhile (another
  path, a link retargeted, a folder made again), writes nothing and answers
  `conflict`. A save that went through but whose layout after failed
  answers `layoutError`, and the editor says it was saved and why the
  layout failed.
  The editor keeps the text and offers Reload (the file as it is now) and
  Overwrite (the same save, `force`, which lets a changed file go but never
  another library). The page's `library_write` is unbound, as before.
- `library_draft` and `set_library_draft` (sent): the editor's text not
  saved, with what it was read from and the id of the editor that keeps
  it, kept as it changes in `<data>/library-draft.json` (private, one
  writer, drained as the app quits; read back only as a private plain file
  of Luna's, no link, at most 8 MB, `secret::read_private`) and forgotten
  once saved, reloaded or thrown away. A forget names the draft's id
  (`{ draft: null, forget }`), decided in the order sent: throwing away a
  draft on offer leaves one another editor has kept since. A clear that
  names no draft is refused and logged: only a named draft is cleared. A window that
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
  and file id, or device and inode), and the yes deletes only that folder:
  under the library's lock the accounts root in Settings is checked to be
  the one asked about, the provider's folder and the account's are opened
  as themselves (any reparse point refused, on Unix on the root's device),
  the account's held to that identity, and it is emptied and removed
  through its handle (`src/held.rs` `remove_tree`), a link inside removed as
  itself and a mount on another device inside refused before anything goes
  (on Unix a bind mount on the same device is not seen: threat model, B5);
  both are held to
  `boundary::account_in` first (no link, inside the accounts root). The question
  names the chats set to the account (`Workspace::chats_on`, up to five and
  how many more), which have nowhere to start once it is gone.

Sync is one acting pass of the core's (`library_sync`), the same as runs
before every session's start, as ADR-0030 has it: Luna's own copies are
written over with the library's or put back, those no longer wanted taken
out, a changed one backed up first, and a place holding a file Luna did not
write is left alone (library.md). The overview it answers carries what the
pass did (`written`, `removed`, `notRemoved`; empty on a look), and the
window says from it how many copies it wrote and took out, how many places
it left alone (`taken`), and, a line each with why, every copy it could not
write (`failed`, or `behind`/`missing` still: saved there while the pass was
at it) or take out. "Every copy Luna manages holds what the library says"
only when there is no such line.

**What the panels hand the window's other parts.** The preferences Settings
writes (`window_prefs`) are the workspace's too (`Workspace::prefs`,
`new_chat_start`): asked for at start, replaced as Settings changes them, so
the new-chat dialog starts its boxes from them in a folder with no start of
its own. A rename of an account renames it in every chat set to it
(`WindowState::rename_account`, as the page's store does), and a rename,
a new or a deleted account, or a new accounts folder, has the workspace list
the accounts again (the chats' sessions start on what it lists). The
workspace takes the answer to a rename, a new or a deleted account on a task
of its own (`rename_account_on_host`, `accounts_changed_by`), so the chats
follow and the list is asked again even when the accounts window was closed
before the answer came.

**Signing in** (`accounts/login.rs`): Sign in, offered on an account whose
limits say signed out or could not be read, opens a window over the Accounts
one with a `TermPane` on the account's login session, as the page's
LoginModal.tsx runs it: `ensure_claude_session` of a bare `claude` (model
`sonnet`, permission `default`) or `ensure_codex_session` with `login: true`,
both in the account's folder on the account's own (`accountPath`), under the
id `login:<cli>:<name>`. The pane tells when its session exits (`Exited`,
with the CLI's exit code, which `pty://exit` carries): the window closes on
a code of 0 (or none readable) and the account list and the account's
limits are asked again; any other code leaves the window up with what the
CLI said until it is closed. Closing it ends the session (`kill_session`),
once its start has been answered (a kill sent sooner finds no session), and
not at all for one that exited. The id is the account's, so a window opened
on it while the last one's kill is unanswered waits (`Ending`, a global) and
starts its own once it is gone: it would attach to the very process being
killed. Esc goes to the CLI while the terminal has the keyboard (the layer's
`Close` propagates), and closes the window otherwise.

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
claude <name>`, `delete <n>`, `signin <n>` (the nth account's login
terminal), `yes` for the library's and the accounts' questions;
`panels::drive`), so a smoke run drives the background window
with no global input. No line answers a question of Settings': Luna's update takes a
person's click, and so does a CLI's rollback.

## Dialogs

New chat, quick chat, delete chat, the helpers popover and the crash screen
are in-app too (`dialogs/`, `dialogs/mod.rs` `DialogLayer`, root.rs
`dialogs`, over the panels' layer): one at a time, over a scrim that takes
every click (`occlude`), in the `LunaDialog` key context, where Esc closes
the dialog (a list open, a helper being watched, a chat being made goes
first) and Enter does what its main button does. A dialog plugs into the
window through `Workspace::seams` (workspace.rs): the New chat button and
Ctrl+N, Ctrl+Shift+N and a chat's Delete, set when the layer is made. When
it closes, the keyboard goes back to the terminal it belongs in.

- **New chat** (`new_chat.rs`, the page's NewChatDialog). Folder (the ones a
  person launched in, a picker for others, remembered once browsed to, Forget
  for one holding no chats), account (which says the CLI), Luna's tools, a
  worktree box, trust, then the CLI's own settings: model, effort and
  permission mode for Claude Code, model, reasoning effort, approval and
  sandbox for Codex. It opens on the folder and account of the last chat
  made, with the boxes as the folder's own start says, else as Settings'
  new-chat defaults (`Workspace::start_in`). Model, effort and modes open on
  what the CLI's own files say for the account in that folder, asked of the
  core as the page asked (`claude_defaults`, `codex_defaults`, the models
  `claude_models`, `codex_models`; choice.rs resolves them, the page's
  `settingsDefaults` ported): effort follows the model (Claude Code keeps it
  per model; a model that takes none runs with none, a level the model lacks
  moves to its own default), a value changed here is for this chat only and
  stays through another account or folder, and a line under each says which
  file it came from. The worktree box shows only where a worktree can be
  made: the dialog asks `chat_folder_check` as the start would ask it, whose
  answer is the folder's refusal and whether it is in git (`inGit`); outside
  git there is no box. It is a flag the CLI is given; Luna makes no worktree. A
  folder the account has not trusted shows the trust box, unticked: Luna
  writes the CLI's own mark (`claude_trust_folder`, `codex_trust_folder`)
  only if it was ticked, before the chat exists, and Claude Code with a
  worktree on is held until it is ticked or the box turned off. Create waits
  for every answer (settings, trust, the folder's check) and goes through
  create.rs, the one path of the dialog, the quick chat and a helper opened
  as a chat: the folder is checked once more as the start would, trust
  written, the chat named `chat N`, listed and seated
  (`Workspace::add_chat`), and the folder and account remembered. A refusal
  or a failed write is said in the dialog and nothing is made. Cut from the
  page's dialog: the wait guard, Codex's hooks box, keeping a start for the
  folder and the auto-handoff (ADR-0030); the start asks for none of them.
- **Quick chat** (create.rs `quick_chat`, Ctrl+Shift+N): today's one-step
  creation. The folder and account of the chat on screen (the peek, the
  active pane, the active chat), else the last chat's, else the first folder
  a person launched in; the files' settings; the folder's start. It opens the
  new-chat dialog instead where it has nothing to go on, where the folder is
  not trusted in the account (whether to trust is the person's to say) or
  where creating fails, so the reason has a place to be said.
- **Delete chat** (`delete.rs`, the page's DeleteChatDialog): the question
  every Delete goes through, the sidebar row's and a pane's menu's. It says
  what goes and what stays (the transcript), lists the chat's helpers
  (`background_helpers`, hanging from the chat through helpers, as the page
  lists them) with what each is doing that a delete would cut off
  (`background_at_stake`), and has the boxes "also delete its helpers" (left
  alone, they answer to no chat and delete themselves once idle) and "also
  stop the ones still working". Each box is a yes of its own; the core holds
  the same line (`agents/teardown.rs`). Delete sends `background_delete` for
  the helpers chosen (`stopWorking`, `ofChat` the chat), `delete_session` for
  the chat (`{ id }`: the session and its attachments) and takes the row off
  every board (`Workspace::remove_chat`). Luna removes no worktree
  (ADR-0030): a chat's that its CLI made stays on disk, and the dialog says
  so. Passing a helper to another chat is cut.
- **Helpers popover** (`helpers.rs`, the page's BackgroundChip and
  HelperView): `dialogs::open_helpers(chat, window, cx)` is its entry point,
  for the status bar's Background count: a chat's helpers (its helpers'
  helpers with them), or all of them with no chat. Working and Idle tabs, the
  asking ones first; each row says its state, setup, last activity, task and
  what it asks, and has **Watch** (its conversation as plain text,
  read-only, tailed through `background_transcript` from a cursor, every
  second while a turn runs and every five between), **Open as chat**
  (after a question: `background_promote`, a row with the helper's own id
  made through create.rs and listed, the core told it is made with
  `background_promoted` and retried as the page retries, then seated; a row
  that could not be made tells the core why and the helper stays its
  agent's; one the core gave up on is let go), **Stop** (`background_kill`)
  and **Delete** (`background_delete`). The list is asked of the core when
  the popover opens and every 2 s while it is up: the page was sent every
  change, the window looks only while someone does. Moving a helper and
  handing a chat's context off are cut.
- **Crash screen** (`crash.rs`): when the window starts and the run before
  did not end by itself, it says so, with the reason, when it was last alive
  and where the log is, and offers Restart window (the tray's: a new window,
  the core and its sessions left alone) and Continue. A render that panics
  cannot be caught (gpui offers no way), so the next start is how it is known:
  the panic hook (log.rs) keeps the panic's text and place in the run's
  record (lifecycle.rs `note_panic`), and the exit watcher writes the exit
  code it read (`ended`, a kill told from a crash by its code); the next
  start's `begin` finds a record that never said goodbye and keeps what the
  two saw, or else what it knows (the freeze, the PC going down), for
  `crash_report`. A person killing Luna in Task Manager shows the same screen,
  with the watcher's words for it.

The host answers the dialogs' commands as the panels' (`dialogs::cmd::CALLED`,
native/dialogs.rs): the defaults, models, trust read and folder check are the
page's own functions with their paths held to boundary.rs, the sessions' and
helpers' commands are the core's, and `crash_report` and `restart_window` are
the native window's alone. The test build's driver takes lines for them
(`dialog new-chat`, `newchat folder <path>`, `newchat account <n>`, `newchat
create`, `dialog quick`, `dialog delete <n or id>`, `deletechat yes`, `dialog
helpers <n or id>`, `helpers watch|stop|delete|open <id>`, `dialog crash`,
`dialogs`; `dialogs::drive`), so a smoke run drives the background window
with no global input; no native picker is opened by a line, `newchat folder`
stands for the folder the picker would return.

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
the fixtures in `panels/demo.rs`, and the dialogs' (`dialog-*`) the same on
`dialogs/demo.rs`; `accounts-signin` opens the login terminal
(`Scene::output` prints the CLI's screen into it) and `accounts-delete` names
the fixture's chats. A shot with nothing drawn fails it. `render_to_image` is behind gpui's test
support, which is why the harness is an exe of its own: the app's exe never
carries it. A new surface adds its scenes to `demo.rs` (or its own module's, as
`status/demo.rs` does: `Scene` carries the host's extra answers and failures,
the events told once the window is up, an `act` on the status bar's
model, and a `window_act` on the window's own views: `title-menu` opens the
title bar's Menu, `sidebar-rename` puts a row's name in edit). The dialogs'
scenes include the new chat with its worktree box ticked
(`dialog-new-chat-worktree`), without it for a folder not in git
(`dialog-new-chat-no-git`), with its account or model list open
(`dialog-new-chat-accounts`, `dialog-new-chat-models`, driver line `newchat
menu account|model`), and a quick chat (`dialog-quick-chat`); the
accounts' their add form (`accounts-add`, driver line `adding`), a name in
edit (`accounts-rename`, `rename <n>`) and the parent's list open
(`accounts-parent`, `menu <id>`); the library's a folder's instructions list
open (`library-folder-menu`); the board's the peek's menu (`peek-menu`); the
helpers' fourteen at work under one chat (`dialog-helpers-many`).

**Fit.** `fit_tests.rs` checks that the window fits small screens: every
light scene opened as the shots open it (its act, menu, click and driver
lines), afresh at each of `SIZES` (their size and display scaling, its first
size and its minimum and the laptop screens the WebView window was measured
on, down to 683x400 at 150%). Every element named with `.selector(..)`
(`selector.rs`, in place of gpui's `debug_selector`) that is drawn must show
whole: inside the window, with some size, and inside the content mask it was
painted under, so a control cut off by its dialog's or panel's edge is
caught though the frame is on screen; and a hit test at its middle must
reach it. gpui's `debug_bounds` give only where an element was laid out; in
the crate's tests `.selector` also draws a probe over the element,
positioned out of the flow, that notes `window.content_mask()` (every
clipping ancestor's bounds intersected) and inserts a hitbox that blocks
nothing (`HitboxBehavior::Normal`), with the order it was laid out in. A
control cut off is wheeled toward (`ScrollEvent`s over the nearest point of
its mask, or the mask's middle or corners where something inside takes the
wheel; never a step longer than the room beside it) and fails only if that
does not bring it whole into view. What a popup or modal covers is meant to
be covered: a scene that opens a frame holds to the hit test only what its
probe laid out after the frame's, inside it. The title bar's and the status
bar's controls must be drawn in every scene; a scene that acts must draw
what it opens, its frame fitting as opened (`OPENS`); a drop-down must
close once its face is wheeled out of view (`SCROLL_AWAY`); what is out of
reach today is listed exactly with why (`KNOWN`), and fails once it is in
reach. The test platform lays text out at 0.6 em a character, a little
wider than Cascadia Mono, so it errs toward failing. Selectors are recorded
only in debug builds: CI's Linux and macOS jobs run it, the release-built
Windows tests do not. `gpui-shots.ps1 -Fit` runs it, then shoots every scene
at those sizes into `test-build/shots/gpui/fit/` (`luna-window-shots --size
WxH`).

The status bar's scenes (`status-*`, `orphan-*`; the clock stands still at
2026-11-19T12:00:00Z): the bar with counts, two accounts and the awake chip
(`status-bar`), with an update on offer, a CLI downloading and one failed
(`status-chips`), every chip at once with a countdown (`status-chips-all`),
the update's confirmation (`status-update-panel`) and its
refusal as unsaved (`status-update-unsaved`), a countdown with its Cancel
(`status-countdown`), the power panel (`status-power-panel`), five accounts
giving detail up (`status-accounts-many`), and the orphan line, as it asks
before it kills (`orphan-line`, `orphan-ask`).

The terminal's scenes (`term-*`) feed a pane the demo session's output
through the fake host: colors and attributes, box drawing and blocks, a
selection, an IME composition, an exited session, and one waiting for the
core. The board's scenes keep the synthetic 0.19 fixture's chats
(`fixtures/storage-0.19.json`, imported as on a first start): each layout
(`chats-1` to `chats-4`), the sidebar collapsed, the peek, a pane's menu
open, and the notice of an import that failed. The chat list's own notices:
a file that could not be read (`notice-read`, with Retry), one that read
only in part and was kept aside (`notice-kept`), and one put aside as no
list (`notice-aside`).

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
Restart asks the host for the state shown. `panels/tests.rs` holds each
panel's controls to the commands and arguments the page sends (the editor's
load, dirty state, check, save bound to its source and failed save, a
conflict's Reload and Overwrite, the draft kept and offered, every
transition that replaces the editor asking first, a skill rename's three
outcomes, Sync's report; new, rename and delete; reach,
audience and each account's instructions; folders; system texts' compare,
reset and adopt; every settings row; a CLI's Check and its pass;
the accounts' add, rename, delete,
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

The status bar's tests (`status/tests.rs`) run the window over the fake
host: the counts from the chats' state and gone with the core, Waiting's jump
and the seams, each account asked in its turn and on the page's cadence (a
minute, three after a 429), the usage sent to the core once per change, the
update through every state (offer, download with its progress, install with
and without the stop, a failed download, a failed stop, the `unsaved:`
refusal with Retry, Install anyway and Cancel, a failed check), the CLI chips,
the freeze and the acknowledged write before an install (the Settings'
Update... taking the same steps through the bar's one confirmation), Retry looking at
the core again, Cancel starting the stopped sessions again, the power chip,
its panel, the countdown's one-second clock and its stopping, the rule
unknown while the core is down, a late answer dropped by epoch and revision,
a failed Cancel with Retry, the tray told once per change, the orphan line
(asking, a changed set asking again, killing only what was asked about, one
look at a time with only the latest answering, none while the core is down,
gone with the core), a refresh that moves only the time drawing nothing, and
the whole bar not redrawing through an hour of idle polls. `usage.rs`, `power.rs` and
`updates.rs` have unit tests of their own (instants, the cadence, the fit,
the chip's words).

`dialogs/tests.rs` holds the dialogs to the commands and arguments the page
sends: the new-chat dialog opening on what the account's files say and
creating through the one path (the flag the CLI is given, the folder
remembered, no trust written unasked and written before the chat when
ticked, Claude Code with a worktree held for it, a refusal keeping Create
shut, no worktree box where none can be made), effort following the model, a
changed value surviving another folder while the boxes start afresh, a Codex
account and auto-review's sandbox; the quick chat made at once, handing
over to the dialog for trust or a failure; delete asking first, keeping the
helpers unless told, each box its own yes; the popover's list, watch, stop,
delete and open as chat (and a row that could not be made); and the crash
screen at start. Their pure rules (`dialogs/choice.rs`, `rows.rs`) have unit
tests of their own, and lifecycle.rs's hold what the crash screen says of a
panic and an exit code.

The terminal's tests (`term/tests.rs`) drive a pane through gpui's own
dispatch and the input handler its element registers, over the fake host, its
output through the real sink and parser thread: keys with modifiers and
DECCKM, Ctrl+C both ways, AltGr text, paste with 2004 off and on and in
order with the keys typed while it reads (encoded by the modes they go
with), a clipboard that does not answer and the queue's bound, files pasted
and dropped, a bitmap per CLI, SGR mouse with overlapping buttons, selection
and copy, the wheel and the scrollback, the IME, the size in cells and
pixels and a size refused, a fresh start, a reattach and what came during
it, an exact reattach by offsets (overlap cut, the tail and its query taken
once, an escape cut by the backlog finished), output missed after the
backlog, a reattach past its bound, failed, or answered late, a core replaced
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
