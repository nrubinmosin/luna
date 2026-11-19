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
(the line to the core, from `core_state` at start and every `core://state`),
`Shell` (the theme: the choice and the window's appearance) and `Workspace`
(the chats and the boards, below). The inbox's
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
| `title_bar.rs` | Luna's name, boards I-V, layouts 1-4, Clear board, Auto-tile, Library, Accounts and Settings (panels below), the theme button, minimize, maximize, close | `Shell`; `Workspace`'s board, layout and auto-tile only |
| `banner.rs` | the core banner | `CoreLink`, `Shell` |
| `sidebar.rs` | the board's chats by folder, or the collapsed strip | `Workspace`, `Shell` |
| `board.rs` | the panes in their layout, their title bars and menus, the peek | `Workspace`, `Shell` |
| `term/` `TermPane` | one session's terminal (below) | `CoreLink`, `Shell`; its parser |
| `panels/` `PanelLayer` | the library, settings or accounts window up, over a scrim | `Shell`; each panel what it shows |

The title bar, the sidebar, the board and each terminal are cached views
(`Entity::cached`): a frame drawn for the banner reuses them, and they lay
out again only when their own size changes. Frames come on demand (vendor/PATCHES.md, patch 3), so an idle window
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

## Chats, boards and the window's state

`Workspace` (`workspace.rs`) holds the window's state (`state.rs`
`WindowState`) and what is only this run's beside it. The state is plain
data with the rules that change it, ported from the page's stores
(chats.store.ts, panes.store.ts) less what 0.20 cut:

- **Chats and folders.** A folder is a path chats were started from, kept
  when it empties (the new-chat dialog offers its `picked` ones). A chat has
  its CLI, account, board (0-4), session id, worktree flag and path, tools,
  guard, model, effort and permission or approval and sandbox, its name and
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
The session's end (sign-out, shutdown) runs the app's quit directly, as
Windows will not wait: the host tries once more there (2 s) and logs an
error if it was not written. The tray's Quit stops the core before it asks,
so a window kept open by a failed save has no core until it is restarted.
`Saver` writes as the page's throttled storage did: a change after a quiet
spell at once, then at most one write per 500 ms (`GAP`), the same text
never twice; a change of the chats themselves (one added or deleted, its
session id, account, board, folder or worktree path: `critical_key`) at
once; what waits is written as the app quits (`workspace::flush`, before
the host's writers drain). Nothing is written before the host's state has
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
answers with what 0.19's WebView window kept as well. The host copies the
files of `<webview>/EBWebView/Default/Local Storage/leveldb` (paths.rs
`webview_dir`, Windows only; all but `LOCK`; plain copies) into a private
folder of its own, `<data>/window/import-<pid>-<hex>`, made with
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
of those the one whose `luna.chats` is a list that imports
(`luna_window::chat_list_size`), not merely the first with a `luna.*` key;
where two or more hold one, the one with the most chats (then folders; the
first of that order on a tie), the others logged by their counts alone.
With none that imports, the first with a `luna.chats` at all, so the import
says why it does not read. Chromium's layout is a key `_<origin>\0<key>` whose
key and value each start with an encoding byte (0 UTF-16LE, decoded lossily
so a cut surrogate costs one character, not the key; 1 Latin-1).
`import.rs` makes the state of `luna.chats` (zustand's `{ state, version }`,
version 2), `luna.panes`, `luna.sidebar`, `luna.newchat`, `luna.delete` and
`luna.notify`; groups past V go to board V (their panes are not kept), chats
with no CLI or account are skipped, and cut fields (colors, handoff marks,
account moves, Codex's hooks without asking, splits) are let go. A list
whose state holds no `folders` array (version 2 and up) is a failure, not an
empty list. The result is written at once, so the next start imports
nothing. An import that fails (a store that does not open, read or hold
still, a list that does not read) starts empty, logs why and shows a line
with **Retry** and OK, and is not written, so Retry and every later start try
again while the list stays empty; a change the person makes is written, and
still leaves it retryable while it has no chats. No WebView store at all
(0.19 never ran there), or one with no `luna.chats` (a profile that kept
only its theme), is nothing to import and says nothing.

**Sessions.** Every `EVERY` (4 s), and once the core is up, the chats with a
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

**Terminals.** The board makes a chat's `TermPane` the first time the chat
is shown, from `Workspace::spec_of` (the page's `startSession`: account path
from `list_accounts`, Claude Code's worktree by name, `handoff` null), and
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
input until its file is read. The title bar's Library, Accounts and
Settings buttons open them (`panels::open`, which the demo scenes and the
test build's driver (below) call too); the status bar takes Settings over
when it comes. The layer starts below the title bar, so the window's own
buttons and the three panel buttons stay in reach, and another button
switches panels.

Each is a module of its own and asks the host for what the page's dialog
asks, by the same command and arguments (`panels::cmd`, held by native.rs's
dispatch test as `cmd::ALL` is):

| Module | Shows | Commands |
| --- | --- | --- |
| `library/` | the library's folder and its sync; its instructions, skills, system texts and folders in a list; the one picked: whom it is meant for (`reach_items`, the page's ReachPicker), whose sessions it is for (chats, leads, helpers), what each account and folder gets, and its file in gpui-component's editor (Markdown highlighted by tree-sitter, a steady caret) | `library_*`, `set_library_draft` (sent) |
| `settings/` | General (sound, the accounts folder), Chats (the new chat's boxes), Agents & trust (helpers, trusted folders), CLIs (version, Check, what the automatic rollback turned away; no version is chosen here, ADR-0030), Updates (Luna's, confirmed with its version and notes; the core's restart) | `window_prefs`, `set_window_prefs` (sent), the helpers', `codex_trusted_roots`, `cli_status`, `cli_versions`, `cli_update_now`, the updater's |
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

**What the panels hand the window's other parts.** The preferences Settings
writes (`window_prefs`) are the workspace's too (`Workspace::prefs`,
`new_chat_start`): asked for at start, replaced as Settings changes them, so
the new-chat dialog starts its boxes from them in a folder with no start of
its own. A rename of an account renames it in every chat set to it
(`WindowState::rename_account`, as the page's store does), and a rename,
a new or a deleted account, or a new accounts folder, has the workspace list
the accounts again (the chats' sessions start on what it lists).

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
the fixtures in `panels/demo.rs`; `accounts-signin` opens the login terminal
(`Scene::output` prints the CLI's screen into it) and `accounts-delete` names
the fixture's chats. A shot with nothing drawn fails it. `render_to_image` is behind gpui's test
support, which is why the harness is an exe of its own: the app's exe never
carries it. A new surface adds its scenes to `demo.rs`.

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
