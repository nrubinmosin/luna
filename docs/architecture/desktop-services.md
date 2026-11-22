# Desktop services

`src-tauri/src/desktop/` provides the Rust services for the GPUI window.
`native/desktop.rs` connects their callbacks to the window, and `lib.rs`
claims the single instance before opening it. See the
[native window](native-window.md#desktop-services) for the host integration.

Updates from 0.19 preserve the installer identity, shortcut and updater
compatibility. Luna keeps its service state in `paths::data_dir()`.

## Key modules

| Path | What it holds |
| --- | --- |
| `identity.rs` | app identifiers and names (`rm.luna` / Luna, `rm.luna.test` / Luna Test) |
| `desktop/tray.rs` | `Tray::new(Actions)`, `Tray::set_awake`, the icon and tooltip |
| `desktop/notify.rs` | `show(Notice)`, `on_click(f)` |
| `desktop/single.rs` | `claim(on_second) -> Claim` and the second launch's focus request |
| `desktop/dialog.rs` | `folder`, `file` and their `_async` forms, `Ask` |
| `desktop/clipboard.rs` | `read() -> Clip`, `write_text` |
| `desktop/window_state.rs` | `load`, `save`, `fit` |
| `desktop/updater.rs` | `check`, `download`, `install`, `verify` |
| `desktop/gsync.rs` | `ensure`, `turn_off`, `undo_from_args`: G-SYNC off for Luna's exe in NVIDIA's driver profiles |
| `windows/installer.nsi` | Windows setup built by makensis |
| `scripts/macos-app.sh` | experimental `Luna.app` bundle for macOS |

## Threads and event loops

GPUI runs the main thread's event loop on Windows and macOS. What each
service asks of it:

- **Tray**: made on the thread whose loop runs, after the app has started;
  on macOS that must be the main thread. `tray-icon`'s menu and icon events go
  to one handler each for the whole process, on that thread; the callbacks
  are `Send + Sync` because the crate asks it. The tray is dropped on the same
  thread. No tray on Linux: `Tray::new` returns an error.
- **Notifications**: on Windows `on_click`'s callback runs on the thread
  Windows calls a toast's activation on; nothing waits for a toast. On macOS
  a notification waits for its answer on a thread of its own, within the
  bounds under "Notifications", and the callback runs there; macOS delivers
  the answer on the main run loop, which GPUI runs.
- **Single instance**: the listener has its own thread (a message-only
  window and its message loop on Windows, a `poll` over the socket and a
  stop socket elsewhere); the callback runs there.
- **Dialogs**: the blocking calls run the modal dialog on the calling thread
  (main thread only on macOS); the `_async` ones return a future the window
  awaits from its loop. A `parent` (any window with raw handles) makes the
  dialog modal to it.
- **Updater**: blocks its thread on `crate::http`; call it off the loop.

Every callback should hand its work to the window's loop and return.

## Services

**Tray.** The icon is `icons/32x32.png` decoded to RGBA; the tooltip is
"Luna"; the menu is Open, Restart window, Quit, its right click's; a left
click opens the window. `set_awake(Some(holders))` puts an amber dot on the
icon and says in the tooltip who holds the PC awake (cut at Windows' 127
characters); `None` puts the plain icon back.

**Notifications.** `Notice { title, body, chat_id, sound }`. On Windows,
notifications use `tauri-winrt-notification` (the crate under
`notify-rust` and the plugin), a toast under the AUMID the installer gives
the Start-menu shortcut (`rm.luna`); run from a cargo target folder, which
has no such shortcut, under PowerShell's, as the plugin does. A click on the
toast's body calls `on_click` with the chat id, through the toast's own
activation callback, and the window brings itself up and opens that chat;
its buttons and its closing are no click. No thread waits on a toast:
`notify-rust`'s `wait_for_response` blocks one until the toast is acted on,
for good when Windows drops it unshown (an AUMID without a shortcut, as the
test build's). A click reaches the running Luna only: one left in the
notification center after Luna has quit does nothing, as today. On macOS, on
`notify-rust`, `NSUserNotification` under the bundle's identifier when Luna
runs from `Luna.app`, which needs no signing, only the bundle LaunchServices
has seen; `notify-rust` gives no callback there, so a thread waits for each
answer (`Waiters`). That thread ends when the notification is acted on or
leaves the notification center (mac-notification-sys looks every half
second); one left there unanswered keeps its thread, which nothing can call
off. So a waiter counts against the four that may wait at once for ten
minutes at most, after which a new notification may wait again, and no more
than 32 threads wait in all; a notification that finds no room goes out
unwaited, and a click on it only brings Luna up. Linux shows it and reports
no click.

**Single instance.** One window per identifier and data dir, which is one
core's: the test build runs beside the Luna in use, and so does a copy of
Luna in another folder. The hand-over is focus only: a second launch tells
the first to bring its window up and carries nothing else, its arguments
included (the plugin passed them on; Luna's window never used them).
Windows: a mutex `Local\<identifier>-<hash of the data dir>-sim` says who is
first; the first keeps a message-only window (class `…-sic`, title `…-siw`)
that takes a WM_COPYDATA tagged `LUNA` from a second launch, which lets it
take the foreground (`AllowSetForegroundWindow`) before it exits. A first
Luna run as administrator makes a mutex a normal second launch may not open
(`ERROR_ACCESS_DENIED`): that launch is second too, and as UIPI keeps its
message out, the failed `SendMessageTimeoutW` goes in its log. Unix: an
exclusive `flock` on `<data>/window/lock`, and the first listens on
`<data>/window/sock` (0600); `window/` is made private (0700; a link there or
another user's folder is refused), the lock is opened without following a
link, a failed chmod fails the claim, and the first takes a connection only
from a peer of its own uid (`SO_PEERCRED`, `getpeereid`) that says `LUNA`
within one deadline of a second. A second launch's connect, nonblocking
(`endpoint::connect_within`), and its write are held to a second all told,
so a first whose backlog is full does not keep it waiting; the first's
thread waits on its socket and on one end of a socket pair, whose other end
the guard drops to stop it, so its stop needs no connection either; the
second's write after its connect is held to what is left of that second. The
claim is lib.rs `run`'s, before the window starts anything; the first's
callback is `desktop::bring_up`, which the running window sets
(native/desktop.rs `Request::Show`). The listener is not authenticated beyond
that: any process on the desktop at the same integrity (Windows), or of the
same user (Unix), can call the window up, which such a process can do to any
of this user's windows anyway; it is acceptable only because the hand-over is
focus only (threat model R9). `claim` makes no claim under `LUNA_DEV_TOKEN`
in the test build (`lifecycle::dev_beside`; any other build ignores it);
in the background (`crate::background`) it claims, but a second launch does
not call the window up, as today; the first logs each hand-over either way. The second launch's `Claim::Second` tells
the caller to exit; the plugin exited for it.

**Dialogs.** On `rfd`, as the plugin. A missing start folder opens at its
nearest folder that is there; extensions are taken as `md`, `.md` or `*.md`.

**Clipboard.** `read` returns what a paste would put in a terminal, the first
of: files (CF_HDROP; Finder's file URLs, file reference URLs resolved through
NSURL), text, an image. Text comes before an image because a spreadsheet's
cells come with a picture of themselves. An image is never read: the window
presses the CLI's image key for it (ADR-0030) and the CLI reads it, so `read`
only asks whether one is on offer (`IsClipboardFormatAvailable` for CF_DIBV5,
CF_DIB and the "PNG" a browser puts there; on macOS the pasteboard's types,
PNG or TIFF), with nothing decoded under the open clipboard. The window
types files as quoted paths, so a file list with a path it cannot type whole
is not taken as files (`typeable_path`): a control character (CR, LF, C1 and
the rest) on either platform, and on Windows, whose paths go in double
quotes, a `"`; POSIX's single quotes hold one. Nor is one of more than 1024
paths or with a path longer than 32,767 UTF-16 units (Windows' longest). A
format that does not read gives way to the next: a CF_HDROP whose `pFiles`
points into its header, whose list runs off its end before its closing NUL,
or whose ANSI names (in the system's code page) do not decode. No format
over 64 MiB is read. `write_text` fills the text's memory before it touches the
clipboard, and opens it owned by a message-only window of its own, since
`SetClipboardData` may fail after `EmptyClipboard` on a clipboard opened
with no owner. The Windows formats are parsed by functions the tests hold to
hand-made bytes; no test reads or writes the real clipboard, which is the
person's. In the test build `LUNA_TEST_CLIPBOARD` names a JSON file the
native host reads and writes in its place (native/desktop.rs `test_clip`),
for a smoke run of the terminal; an image or nothing there reads as the text
`[test clipboard: image key]`, since the CLI's real key would read the
person's clipboard. That stand-in is compiled into the test build alone.

**Window state.** `<data>/window-state.json` holds the window's normal
bounds and whether it is maximized, in the coordinates the toolkit gives its
displays in. `fit` holds them to the displays there are now: the window lands
on the display it overlaps most, or centered on the primary when it overlaps
none, no smaller than the minimum and no larger than that display, and
wholly on it. Nothing is saved or restored in the background. What 0.19's
plugin saved under the identifier is not read. The file is the place's
alone: one there that does not read as a place is renamed
`window-state.unreadable-<secs>-<hex>.json` and logged before the place is
written, and where it cannot be renamed the place is not written
(`window_state::save_to`).

**G-SYNC** (`desktop/gsync.rs`, Windows). With G-SYNC on for windowed apps,
or Luna's window covering its monitor (maximized with the taskbar
auto-hidden), the monitor's refresh follows Luna's presents, and as Luna
draws only when something changes the refresh falls to the panel's VRR floor
and everything on that monitor stutters. No DXGI, Direct3D or NVAPI call lets
a window opt out at run time; NVIDIA's driver leaves browsers, Explorer,
Office and Windows Terminal out through its profile database, whose profiles
for their exes set "G-SYNC - Application State" (`VRR_APP_OVERRIDE`,
0x10A879CF) to Force Off (1). So the window, as it starts, on a thread of its
own, loads `nvapi64.dll` from System32 (no other folder), and where there is
an NVIDIA driver opens a session on its profiles. It never changes a profile
it did not make without the person's yes: the one the driver resolves
`luna.exe` to by its bare name (NVIDIA ships one named "Luna", a game's)
applies to every exe of that name.
Where the exe is off already, or a profile holds the exe's own full path (a
person's choice), it writes nothing. Else it makes its own profile, named
"Luna native window (G-SYNC off)" and eight random hex digits (no comma: the
driver refuses one in a profile name, see "Seen on" below), with the
running exe by its full path (`c:/…/luna.exe`, which nvapi.h recommends and
the driver matches before a bare name) and the setting at Force Off, and asks
the session once more which profile the full path resolves to
(`NvAPI_DRS_FindApplicationByName`, which for a full path answers with the
profile the driver will apply); anything but Luna's entry, and nothing is
saved. The driver takes no entry for an exe whose name a profile already has
as a bare name: `NvAPI_DRS_CreateApplication` answers
`NVAPI_EXECUTABLE_ALREADY_IN_USE` (-167), for the full path as for every
other form of the entry tried. So where NVIDIA's "Luna" is in the database, a `luna.exe`
gets no profile of Luna's and nothing is saved; see "Seen on" below. Where
the profile that has the name is the one the exe resolves to, by that name,
the window is told (`Outcome::Ask`, `desktop://gsync-offer` with the
profile's name and the exe name it holds) and asks the person (below); else
it says which profile has the name (`refused_entry`). A path that is no Unicode (an unpaired surrogate) or longer than an
`NvAPI_UnicodeString` holds is not asked about at all: NVAPI would answer
for another path.

**The profile that holds the name.** The offer is a notice with Turn off,
Not now (for this run) and Don't ask again (the window's preference
`gsyncAsk`, in `window.json`; Settings, General turns it back on, and the
offer still up this run shows again). It is shown only once the window's
preferences have settled and say to ask. Turn off calls `gsync_turn_off`
`{ profile }` (`turn_off`), which takes nothing else from the window: under
the same lock, in a session of its own, it resolves the exe's full path
again and acts only where that gives the named profile by an entry of the
exe's bare name; where the setting already applies there at Force Off it
writes nothing. Else it sets `VRR_APP_OVERRIDE` at Force Off as that
profile's own value (`NvAPI_DRS_SetSetting`), and nothing else: no entry, no
other setting, no profile made or removed. It asks the session again that
the exe now gets Force Off from that profile, writes
`<data>/nvidia-setting.json` (`SetRecord`: the profile, its own value
before or none where the driver's applied, the flavor, pending), flushed,
saves, and confirms the record. A save that fails takes the record back. The
record is kept and read under the rules of `nvidia-profile.json` (below). A
start that finds it pending settles it from the database (Force Off as the
profile's own value means the save went through); one that finds it
confirmed, with the exe still resolving to that profile, neither sets nor
asks again, so a later choice in the NVIDIA Control Panel stands. The
uninstaller's undo puts it back only while the profile's own value is still
Force Off: `NvAPI_DRS_RestoreProfileDefaultSetting` where the driver's value
applied before, else the person's earlier value set again; a value changed
since, or a profile gone, is left, and the record goes. A predefined profile
is never removed, and no other setting in it is touched.

Luna's application entries are its mark: each has a friendly name "Luna
(…)" with a marker of 16 random hex digits, the record's, and nothing else
set (no launcher, file in the folder, command line or flag). A profile is
Luna's only while it is not predefined and holds such an entry for a path
the record has; a profile of the recorded name without one (made by someone
after Luna's was removed) is never written or removed, and an entry a person
added to Luna's profile, or one changed since, is never taken for Luna's.

What it made is in `<data>/nvidia-profile.json` (`Record`: the profile's
name, the marker, the build's flavor, whether the profile is known to be
saved, the exe paths it holds by Luna's doing, and the one about to be
saved). The window's start, a Turn off and the undo each hold `<data>/nvidia-profile.lock`
from before they read the record to after they are done with the driver's
database, so neither reads one while the other changes it; the undo reads
the record before it loads the database. Letting go, each removes the lock
file where neither record is left (and, for the undo, all of Luna's is gone) and
no other process has it open (`FileStore::release`); one that was waiting
opens a fresh one. The record is written, flushed to
disk, before `NvAPI_DRS_SaveSettings` and confirmed after it; a start that
finds one still pending settles it from the database: Luna's entry for that
exe there means the save went through, none means it did not and the record
goes. A start that finds no record, where the exe's full path resolves to
Luna's entry in a profile of Luna's naming (the record's rename undone by a
power cut after the save), takes that profile back into a new record. A
confirmed record that has the running exe is the end of it: nothing is
written again, so a choice made since in the NVIDIA Control Panel stands. An
exe at a path the record lacks (a portable copy moved) gets that path added
to the same profile, its setting as it now is; a profile that is gone, or no
longer Luna's, is not made again. The record is acted on only where the data
folder is no link or junction and the record a private file of one name,
owned by this user's own SID (not Administrators, SYSTEM or
TrustedInstaller), opened without following a link
(`secret::read_own_private`), written by a build of the same flavor and
naming a profile of Luna's naming (for a release, the earlier "Luna (native
window, G-SYNC off) …" counts too: the driver never let a profile of that
name be made, so only a record may name one); else it is left as it is and
the window shows the notice. A window run as administrator leaves the profiles alone
and says so in the log: Windows gives what an elevated admin's run makes to
Administrators, so its record would not be read, and `paths::data_dir`,
which takes the exe's folder where the process may write there, may not be
the folder Luna has when it runs as the person.

The uninstaller, not an update, runs `luna --nvidia-undo` once its running
check knows the window closed and then the core's stop answered that no core
runs; where either is not known, the undo is skipped and said. The undo, as
the person (an elevated one refuses: the uninstaller is `RequestExecutionLevel
user`, so it runs with the window's token and finds the window's data folder),
clears the record's profile and the one its own exe's full path resolves to
where that is Luna's (a record that a power cut rolled back may not know of
it). From each it removes the entries of Luna's making by their marker,
whatever their path, so a moved exe's entry the record lost goes too; the
profile whole while it holds only Luna's entries and setting, all its entries
read; where a person added a setting, only Luna's entries go and its setting
with them unless the person changed it; where they added an exe, or the
entries could not all be read, only Luna's entries go. It then reads the
session again: where an entry of Luna's is still there, the entries could not
all be read, or the exe still resolves to Luna's profile, it exits 4 with a
line the uninstaller shows and keeps the record. Else the record goes; one
that cannot be removed then is logged, and finds nothing to do the next time.
A person who starts Luna again while the uninstaller runs is out of its
reach: the window's start and the undo do not overlap (the lock), but what
that start makes after the undo stays, and the exe is gone after it. The
undo of the profile and of the setting (above) run in one session, and the
uninstaller is told the worse of the two ends. Where the driver refuses (`NVAPI_INVALID_USER_PRIVILEGE`, as it
may for a process that is not elevated, and the installer is not, or
`NVAPI_EXECUTABLE_ALREADY_IN_USE`, above), or any
call fails, the window logs one line naming the NVAPI call and its status
(`Fault`: `NvAPI_DRS_CreateApplication: NVAPI_EXECUTABLE_ALREADY_IN_USE
(-167)`) and shows a one-line notice pointing to
the user guide's Fixed Refresh setting. Every start that opens a session
also logs the calls it made, in order, each with its status (`Trail`); the
undo does the same. The undo exits 2 (administrator
rights), 3 (record refused, a window's start held the lock 30 seconds, or
run as administrator), 4 (something of Luna's may be left) or 1, with a line
the uninstaller shows. The test build only reads, unless
`LUNA_TEST_NVIDIA=write` (a live test's), and then names its profile "Luna
test build (G-SYNC off) …"; it acts on no release's record, nor a release on
its. NVAPI's ids and structure layouts are those of NVIDIA's MIT-licensed
headers, no SDK linked; the structures' sizes are held to the headers' in
unit tests, and the record's logic is tested over a fake of the calls
(`Drs`). For the variable refresh this does not reach, the window keeps
presenting at the display's rate for a second after the person's input
(vendor/PATCHES.md, patch 4).

Seen on NVIDIA driver 621.24 (branch r621_20, Windows 11, a live test of the
test build on 2026-11-22): a save by a process that is not elevated was
allowed. The full-path entry won over NVIDIA's predefined bare-name "Luna"
profile: `NvAPI_DRS_FindApplicationByName` resolved the test exe's full path
to Luna's profile, and did so in the session before the save, while another
`luna.exe` still resolved to "Luna". The driver stored the entry's path
lowercased (`e:/…/luna-test.exe`), kept the marker in its friendly name and
left its launcher, file in the folder, command line and flags empty; a
profile's `numOfSettings` counted its own settings only (1). The undo put
the database back as NVAPI reads it, every profile, entry and setting byte
for byte, but the driver's files under `%ProgramData%\NVIDIA
Corporation\Drs` (`nvdrsdb0.bin`, `nvdrsdb1.bin`) were rewritten by the
saves and do not hash as before.

Seen on the same driver, the same day, with an exe named `luna.exe` (the
test build copied under that name, and a probe that never saved): its full
path resolves to NVIDIA's predefined "Luna", whose one entry is the bare
`luna.exe` (no file in the folder). `NvAPI_DRS_CreateProfile` succeeds and
`NvAPI_DRS_CreateApplication` refuses Luna's entry with
`NVAPI_EXECUTABLE_ALREADY_IN_USE` (-167): for the full path with either
slash, for the bare name with a file in the folder, with a command line
(`isCommandLine`), a launcher, `isMetro` or `isPredefined` set, the same.
The rule is the exe's name, not the profile's being predefined: a full path
is refused wherever a profile, a person's included and the same one, has
that name as a bare name, also with a file in the folder (`launcher.exe`,
which NVIDIA's profiles hold only so, is refused too), while two full paths
of one name in two profiles of a person's are both taken, each resolving to
its own. The process's own exe name changes nothing (the probe ran as
`luna.exe` and as another name). So Luna cannot have Force Off for a
`luna.exe` beside NVIDIA's "Luna" without changing that profile, which would
reach every `luna.exe`; it is refused for every user alike, so elevation
would not help (not tried: the refusal is not `NVAPI_INVALID_USER_PRIVILEGE`).
The 0.20.1 release logged `NVAPI error -1` (`NVAPI_ERROR`) instead, once,
0.6 s after its update restarted it; it did not log which call, and the
same steps on the same path, replayed since, answer -167 at
`NvAPI_DRS_CreateApplication` every time, so that -1 was not reproduced, and
which call answered it was not known then; the trail now in the log would
say (the profile's name, below, answers -1 at `NvAPI_DRS_CreateProfile`). A full read-only dump of the
database through NVAPI (every profile, entry and setting) was the same
before and after.

Seen on the same driver on 2026-11-23, with a probe in a session that never
saved: `NvAPI_DRS_CreateProfile` answers `NVAPI_ERROR` (-1) for a name with
a comma. "Luna (native window, G-SYNC off) 0123abcd", the release's name
before, was refused; "Luna (native window G-SYNC off) 0123abcd" was made.
So 0.20.1 and 0.20.2 made no profile, and asked nothing, on release builds,
while the test build's "Luna test build (G-SYNC off) …" was made, took
`luna-test.exe` and saved, the same day on the same driver. A read-only dump
of the database had 8056 profiles, none with a comma in its name; titles
that have one are stored without it ("Warhammer 40000: …"). Luna's profile
name now holds only characters seen in names the driver took.

Seen on the same driver on 2026-11-23, unelevated: NVIDIA's "Luna" has no
`VRR_APP_OVERRIDE` of its own (`NvAPI_DRS_GetSetting` answers it from the
global profile, location 1, value 0). `NvAPI_DRS_SetSetting` at Force Off on
"Luna" and `NvAPI_DRS_SaveSettings` both answered `NVAPI_OK`, and a
`luna.exe` then resolved to "Luna" with the value 1.
`NvAPI_DRS_RestoreProfileDefaultSetting` then took the profile's own value
away (the global's 0 again), and asked again with nothing of the profile's
own it answered `NVAPI_SETTING_NOT_FOUND` (-160). A full read-only dump of
the database was the same before the set and after the restore.

## Updater

Compatible with tauri-plugin-updater both ways, so a 0.19 window updates onto
0.20 and 0.20 onto later releases:

- **The same endpoint and key**: `endpoint()` and `pubkey()`, read from
  `src-tauri/updater.json`, compiled in. The release scripts read the same file
  (`release-lib.ps1` `Get-UpdaterEndpoint`, `Get-UpdaterPubkey`), so a release
  is signed for, and published where, this build looks; `release-lib.test.ps1`
  checks that `updater.rs` reads that file. The file is a fingerprint input.
- **The same manifest**: `latest.json` as `release-lib.ps1`
  `New-UpdaterManifest` writes it (`version`, `core_fingerprint`, `notes`,
  `pub_date`, `platforms.windows-x86_64.{url,signature}`), or the plugin's
  other shape with `url` and `signature` at the top. A `v` before the version
  is taken; a `pub_date` must be RFC 3339. The release is offered when its
  version is newer by semver; one that is not is no update, whatever it has
  for this platform. The platform key is `<os>-<arch>-nsis`, then
  `<os>-<arch>`. The test build is never offered one.
- **The same signature check**: minisign through the plugin's own
  `minisign-verify`, prehashed and legacy signatures alike, then the version
  in the trusted comment against the release's. Stricter in one thing: a
  signature that names no version is refused (the plugin lets it pass unless
  told otherwise), as every Luna signature names one.
- **The core fingerprint from the signature**: the release's signing
  (`release-lib.ps1` `New-UpdaterSignature`) adds
  `core_fingerprint:<16 hex digits>` to the trusted comment after Tauri's
  `timestamp`, `file` and `version`; 0.19's
  plugin reads `version:` alone and passes over it. `verify` returns it
  (`Signed`), in build.rs's form or not at all, `download` keeps it with the
  staged setup (`Downloaded::core_fingerprint`), and `update_download`
  answers it as `coreFingerprint`. The window decides on that one whether
  the sessions stop (`Vitals::proceed`), and the host starts an install
  that kept them (`update_install`'s `keep`) only where that signed core is
  the one they run in (native/desktop.rs `check_kept`). `latest.json`'s
  `core_fingerprint`, which is not signed, only words the confirmation
  before the download, and what the confirmation said holds. The install
  goes only on the confirmation as it was drawn (`Vitals::install` takes
  the panel's `Confirmation`: the core it was worked out against and the
  sessions the core's board listed then, by id), so a click on a panel
  drawn for another core, or on one that could not say what runs (no core
  up, or its board not heard yet: it then promises nothing, "nothing
  stops" least of all, and offers no install), starts nothing. The whole
  update is held to that confirmation, a Retry's or an Install anyway's
  too, and nothing is stopped beyond what it showed: the sessions it agreed
  to stop are none where it said they keep running or none runs, else the
  ones it counted. Where the window sees otherwise when the download is
  through (another core up, or a session on the board the panel did not
  count), the update fails with the offer kept and the panel, drawn again,
  says what an install would do now. Beyond what the window sees, the stop
  (`stop_all_sessions { core, confirmed }`) carries both: the host sends it
  to the core named alone, refusing with `CORE_CHANGED` a connection to
  another one, found at once or once a core still starting is up
  (corelink.rs `stop_all_sessions`), and the core, its start gate closed
  and the pane launches under way sealed off, refuses it before it stops
  anything where it would end a session or a helper's turn not among those
  confirmed, calling the update off and opening the gate again
  (pty/control.rs `stopped_for_update`). After an install that could not
  write first, Retry and Install anyway go only in the core the sessions
  were stopped or kept in (`Vitals::core_changed`), else the update is
  called off and the confirmation shown again; and the host refuses an
  install that kept them with `CORE_CHANGED` where that core is no longer
  the one up, so the window lets the offer go. The release scripts still write
  `latest.json`'s for 0.19, which takes it from there.
- **Stricter than the plugin**: only an `https://` URL; nothing older than
  or the same as the running build is downloaded or installed, whoever asks
  (`parse` offers none, and `download` and `install` refuse one too); the
  setup must be an NSIS setup as well as signed (`is_nsis_setup`): its stub
  a 32-bit x86 GUI executable (`MZ`, `PE\0\0`, machine 0x014C, PE32,
  subsystem 2, no DLL), as every NSIS setup's is whatever it installs (0.19's
  and the native one alike), and right where its sections' data ends NSIS's
  first header (`0xDEADBEEF`, `NullsoftInst`) with as many bytes after it as
  it says. That what the setup installs is AMD64 is held at build time:
  `build-windows.ps1 -Installer` refuses a `luna.exe` whose PE machine is not
  0x8664 before makensis runs.
- **Staged in the data dir**: the installer goes to `<data>/updates/`, made
  private (`secret::private_dir`: a link or junction there, or another
  user's folder, is refused), under a data dir that is no reparse point
  (link, junction or other) and belongs to this user, Administrators, SYSTEM
  or TrustedInstaller (root on Unix), checked before the download, before
  the file is written and again at the install (`check_root`); under the
  app's own name, `Luna_<version>_x64-setup.exe`, never the URL's name or
  extension (`latest.json` is not signed), written private (`write_private`)
  only after it checked out. The plugin wrote it to the system's temp
  folder, and left it there (`%TEMP%\<product>-<version>-updater-*`). As
  the window starts, a thread of its own clears what earlier updates staged
  (`clear_downloads_in_background`, native.rs `start`), and says in the log
  once what it deleted and, in one line, what it could not do (a data dir
  or `updates/` it refuses, a file it could not read or delete). Only the
  setups Luna staged: staging writes a record beside the setup first,
  `<setup>.staged` (`Stage::stage_setup`), and the clearing takes a file of
  a name `file_name` gives, exactly, only with its record, both private
  plain files of Luna's, the setup staged an hour or more ago
  (`STAGED_KEEP`: another window, such as one of another Windows session
  on the same data dir, may be about to start it) and open nowhere else
  without sharing deletion (a setup `install` holds or that runs is left);
  the record goes with it. Anything else in `updates/` stays, a file of
  that name without its record among them (one a person keeps there, or
  one in a folder of that name beside a copy of Luna started elsewhere,
  whose ACL it may inherit as a private one). All through handles, as the
  staging writes: the data dir and `updates/` opened as themselves and
  checked (a link, junction or other reparse point at either refused,
  never followed), the names read from the folder's handle, each file
  opened relative to it, checked and deleted through its own handle (on
  Unix unlinked from the held folder's descriptor). No `updates/`, nothing
  to do; nothing is made. What is left open, accepted: on Unix (not a
  released platform) the entry is unlinked by name after the file was
  checked through its descriptor, so a staging of the same name between
  the two loses its new file; on Windows a clearing holding an old setup
  open can make a staging of that same name fail its rename onto it, and
  the next check offers the update again; a record whose setup was never
  written (its write failed) stays until a staging of that name replaces
  both.
- **Held through the write** (`Pin`): from the data dir's last check until
  the written file is checked, the data dir and `updates/` are open as
  themselves (a link at their name not followed); on Windows sharing reads
  and writes but not deletion, so neither they nor a folder above them can be
  renamed or deleted meanwhile. The file is taken only if it is the one at
  its name in the folder so held: on Windows the path its own handle
  resolves to is that folder's, resolved the same way, and its name; on Unix
  the file reached by its name from the folder's descriptor (`openat`) has
  the device and inode of the one at the path; and a private file of
  Luna's. Otherwise the download fails and the file written is left where
  it is.
- **A local fixed drive only** (Windows): the data dir, as Windows resolves
  it through any link above it, must be on a volume `GetDriveTypeW` calls
  fixed (`fixed_drive`); a share (`\\server\share\…`, refused before the
  network is asked), a mapped network drive or a removable one is refused
  before the download and again at the install, and the setup is not
  started from a path that is not on one. What is on a share is the share's
  to change under Luna. The download's error, in the log as `updater:
  download failed: <data dir> is on a network share: no update is staged
  under it or started from it` (or `on a network drive`, `on a removable
  drive`), is what `update_download` answers.
- **A data dir that is a reparse point disables self-update**: a link, a
  junction or any other reparse point, a OneDrive folder's (files on demand)
  among them, at the data dir itself. The log says `updater: download
  failed: <data dir> is not a plain folder (a link, a junction, another
  reparse point such as a OneDrive folder's, or a file is there): no update
  is staged under it`; Luna is updated by running the setup by hand, or
  from a data folder that is none.
- **No gap between the check and the start**: `install` opens the staged
  file as itself (a link at its name is not followed), sharing reads alone,
  so nobody may write, rename or delete it while it is open; holds it to a
  private plain file of one name (`secret::private_file`); checks the bytes
  read through that handle as `download` did; and starts it with
  `ShellExecuteExW` (`SEE_MASK_NOCLOSEPROCESS`, `SEE_MASK_NOASYNC`, so it
  returns once the process is there) before letting the handle go, by the
  path Windows gives that handle (`GetFinalPathNameByHandleW`, as `C:\…` or
  `\\server\…`, refused where that form would read differently): where
  the file is, through no link or junction, so a junction above the data dir
  retargeted after the check leads the start nowhere else. While the file is
  open, neither it nor a folder on that path can be renamed (a test holds
  Windows to it), so that path keeps naming the file checked.
- **The same start**: "open", `/P /UPDATE /R /ARGS <this run's arguments>`,
  each argument quoted Windows' way and a `/` quoted too (the plugin's
  escaping); the caller then exits at once, as the plugin's `exit(0)` did.
  `/P` is passive (a progress bar only), `/UPDATE` installs in place without
  uninstalling first or touching the shortcuts, `/R` starts Luna again,
  `/ARGS` with those arguments. Only arguments that reach the restarted Luna
  as they are go (`restart_args`): one with a `"` (which NSIS's own start-up
  reads differently from Windows), one that opens with `/` (which it takes
  for an option, `/S` or `/NCRC`), one with a control character and one
  that is not Unicode are left out, said in the log. Nor are they cut short:
  NSIS reads its command line into a string of 1024 UTF-16 units
  (`NSIS_MAX_STRLEN`, far under Windows' 32,767, its NUL among them) and drops
  the rest, and the setup takes one that fills it (1023 units) for one that
  was cut (`ArgsCut`), so when the whole of it,
  `"<setup>" /P /UPDATE /R /ARGS <arguments>`, would be over 1022 units
  (`SETUP_CMDLINE_MAX` less its NUL and that unit), none of the arguments
  go, said in the log, and Luna restarts without them (`parameters_for`); a
  setup path too long for even the bare update is an error. Off Windows an
  update is installed by hand.

The window handles the update chip, release notes, core
fingerprint compared to decide whether the sessions keep running, the stop of
the sessions before the install (power-and-lifecycle.md, "Updates"). The
native window reaches the updater through three commands of its host
(native/desktop.rs: `update_check`, `update_download { generation }` with
`update://progress`, `update_install { generation, reason, anyway }`), which
hold the release the check offered and the installer the download staged, so
the window names neither a URL nor a file, only the number of the offer it
confirmed: once a check has put another up, both refuse with
`desktop::OFFER_CHANGED`, which the window takes as the offer gone and checks
again at the next click; `update_install` notes the exit
(`lifecycle::exit_for_install`), takes it back if the setup does not start, and
otherwise quits the window's app. Before the setup starts it writes what the
host holds (the chat list, the theme, the window's place, the preferences,
the library editor's draft): the setup's
Restart Manager ends Luna by force (`installer.nsi`), so a change still
waiting would be lost. Where that fails the install is refused with an error
beginning `unsaved: ` and nothing is started; the caller offers Retry (the
same call) and Install anyway (`anyway: true`), the quit path's choice. The
caller sends the window's own waiting state first (`Workspace::quit`'s
flush). The native window's status bar does both: it flushes, asks, and on
the refusal offers Retry, Install anyway and Cancel
(native-window.md, "The status bar and the orphan line").

## Installer

`windows/installer.nsi`, built with makensis (NSIS 3.11, in the Windows build
image) by `scripts/build-windows.ps1 -Installer`, into the folder and under
the name Tauri's bundler gave 0.19's setup
(`…/release/bundle/nsis/Luna_<version>_x64-setup.exe`). It includes
`windows/hooks.nsh` unchanged and calls its hooks where Tauri's template does,
but for the uninstall's order: the install folder's DACL before the first
file; on uninstall the running check first, which closes the window, then
the core's stop and the `core\` copies' removal (so no window still open can
start a core after the stop), then, once the running check knows the window
closed, the core's stop answered that no core runs and it is no update,
`luna --nvidia-undo` (G-SYNC, above; a skip or a refusal is said in the log
and a message), the WebView
profile's removal when the app data is deleted. A pre-release version (`x.y.z-pre`, as
`release.ps1 -Bump` allows) names the setup in full; Windows' version
resource gets `x.y.z.0`.

It is adapted from Tauri's template, tauri-bundler 2.10.5's (the one the
Tauri CLI 2.12.5 embeds), MIT OR Apache-2.0, and reproduces parts of it word
for word: the pages, the sections, the shortcut, pin and AUMID macros and
the Restart Manager check. Its header says so and lists what changed; the
license texts are in `third-party-licenses/installer/tauri-nsis-template/`
and its entry in `components.json` puts them in the installed
`THIRD-PARTY-NOTICES.txt`.

Compared with Tauri's template:

| | Tauri's (0.19) | Native |
| --- | --- | --- |
| Product, identifier, publisher, exe | `Luna`, `rm.luna`, `luna`, `luna.exe` | the same |
| Mode | per user (`currentUser`), `RequestExecutionLevel user` | the same |
| Folder | `%LOCALAPPDATA%\Luna`, or `HKCU\Software\luna\Luna`'s last one, or `/D=` | the same |
| Uninstall key | `HKCU\…\Uninstall\Luna`, the same values (DisplayName, DisplayIcon, DisplayVersion, Publisher, InstallLocation, UninstallString, NoModify, NoRepair, EstimatedSize, MainBinaryName) | the same; EstimatedSize counts the exe, uninstaller and notices, not the folder, which is the data folder too |
| Shortcuts | `%APPDATA%\…\Start Menu\Programs\Luna.lnk` with AUMID `rm.luna`; a desktop one on a passive or silent first install, or from the finish page | the same |
| Command line | `/S`, `/P`, `/UPDATE`, `/R`, `/ARGS`, `/NS`, `/D=` | the same |
| A running Luna | Restart Manager: asked, or closed at once when passive or silent | the same |
| Uninstall | shortcuts unpinned and removed, the AUMID's jump list cleared, the Run entry removed; "Delete the application data" removes the install-location key and `%APPDATA%`/`%LOCALAPPDATA%\rm.luna` | the same |
| WebView2 | bootstrapped when missing | gone: no WebView |
| A previous install | a page offering to uninstall it first (not on an update) | always installed in place, as an update is |
| Helper DLL | `nsis_tauri_utils` (semver compare, de-elevated start, string replace), downloaded on every build | none: the restart is `LunaRunAsUser`, the desktop shell's `IShellDispatch2::ShellExecute` called through the System plug-in, so Luna runs as the shell does and in its environment, asked only when the shell's own token is not elevated (`TokenElevation` of the desktop window's process, `LunaElevation`: an explorer run elevated, as an administrator's is with UAC off, would start Luna elevated too); otherwise, or with no shell to ask, started directly only when the setup's own token is not elevated; when it is, or either token cannot be read, not started at all, and the details say so |
| `/ARGS` | read with `${GetOptions}`, which takes the quotes off a tail that opens and ends with a quoted argument, and the other options read over the whole command line, tail included | the tail after ` /ARGS` handed to Luna as it stands, the options read from the part before it (`SplitCmdLine`); a command line that fills NSIS's 1024-unit string, and so may have been cut, restarts Luna with no arguments, as does a direct start whose command line would not fit, and the details say so |
| The restart after an update (`/R`) | from `.onInstSuccess` | from the end of the install section. With a window (the updater's `/P`), `.onInstSuccess` runs while that window handles a message the install thread sent it, where COM refuses any call to another process (`RPC_E_CANTCALLOUT_ININPUTSYNCCALL`): the shell was never asked, and Luna started as the setup's own child, in the updating Luna's environment (seen in the 0.19 to 0.20 rehearsal). The section runs on the install thread, or the setup's only one when silent, where the call goes through |
| A file or registry value not written | NSIS's error flag raised and not read: the setup finishes, and an update starts Luna again over a partial install | the flag cleared before Luna's files and before the install's record and read after each: raised, the setup stops there (`LunaRefuse`: a box unless silent, the details, exit code 2) and Luna is not started again; an update run passive (`/P`) shows the box, as its other refusals do |

**What NSIS itself reads of the command line.** Before any of the script
runs, NSIS's start-up walks the whole command line, `/ARGS` tail included,
for its own `/S`, `/NCRC` and `/D=`, knowing quotes but not Windows'
backslash escapes; a `/D=` it finds ends the command line there and sets the
install folder. The native updater passes no argument that could start one
(`restart_args`), but a 0.19 window's updater (tauri-plugin-updater) passes
its own arguments as they are, quoted Windows' way: an argument holding `"`
puts NSIS's reading out of step with Windows', and what follows it in the
tail can then be read as one of those options. The ordinary 0.19 update is
untouched by this: Luna is started with no arguments (the Start-menu or
desktop shortcut, the setup's own restart) or, after the tray's Restart
window, with `--after <pid>`, neither holding a quote, so its updater runs
the setup with `/P /UPDATE /R /ARGS` and at most those after it. Only a 0.19
Luna that was itself started with such an argument, by whoever started it,
carries one; and whoever can start Luna with arguments of their choosing
already runs as this user.

**Over 0.19.** The 0.19 window downloads the 0.20 setup (`latest.json`
unchanged in form), checks its signature with the same key, stops the
sessions unless the core fingerprint matches, and runs it with
`/P /UPDATE /R /ARGS`. The native installer finds the folder in
`HKCU\Software\luna\Luna`, closes what is left of Luna, makes the folder
private again (hooks.nsh), overwrites `luna.exe`, `uninstall.exe` and the
notices in place, rewrites the same uninstall key with the new version,
leaves the shortcut and its pins alone, and starts the new `luna.exe`. From
then on `uninstall.exe` is the native one, which removes what either
installed. The data folder is untouched; the `webview\` profile 0.19 left
stays until the app data is deleted on uninstall.

**What a partial install leaves, accepted.** The setup's checks of NSIS's
error flag (the table above) stop it before the restart, not undo what
was written: a first install whose uninstaller could not be written leaves
a folder with Luna's files and no record of the install, which the same
setup, run again for that folder, refuses as not Luna's (hooks.nsh); a
failure in the record alone leaves `uninstall.exe` in place, which still
runs when started directly; and a file NSIS fails to extract mid-way can
stop the setup in NSIS's own way (its Abort), before `LunaRefuse` says
anything.

## macOS bundle

`scripts/macos-app.sh <exe> <version> <out-dir>` writes `Luna.app`:
`Contents/MacOS/luna`, `Info.plist` (identifier `rm.luna`, name Luna, the
version as both version strings, `LSMinimumSystemVersion` 10.15.7, Retina),
`PkgInfo`, and `Resources/icon.icns` made from `icons/icon.png` with sips
and iconutil. It is sealed with an ad-hoc signature (`codesign -s -`, no
identity), so that macOS calls a downloaded copy one from an unidentified
developer rather than damaged. `LUNA_APP_DRY_RUN=1` skips the macOS-only
tools, which is how `scripts/macos-app.test.sh` checks it in the gate.

## For the release pipeline

`build-windows.ps1 -Installer` builds the Windows setup from `installer.nsi`.
The release pipeline keeps the setup's asset name and updater manifest format
so installed 0.19 copies can consume them. See [Releasing](../releasing.md)
for signing and publication.

## Failure modes

- **Toasts do not show** when the exe runs without a shortcut carrying its
  AUMID (a copy run from another folder than the installer's): Windows drops
  a toast of an unknown AUMID. As with the plugin. An installed Luna has one:
  the installer's Start-menu shortcut `Luna.lnk`, AUMID `rm.luna`, as 0.19's
  had. The test build's is the "Luna Test" shortcut `test-build.ps1 -Run`
  writes into the Start menu, AUMID `rm.luna.test`.
- **The exe needs a manifest for common controls v6**: `tray-icon`'s menus
  (muda) may import comctl32 functions that only load with it, and so does
  `SetWindowSubclass` (sysmenu.rs). Every mode of the exe
  carries gpui's manifest, which asks for it (native-window.md,
  "Platforms"). The unit tests never make a tray for that reason.
- **No tray** (`Tray::new` fails, or Linux): the native window says so in
  the log and closing it quits, as there would be nothing to bring it back
  from.
- **A second launch that finds no listener** (the first still starting, or
  hung) waits a second, says so in the log and exits anyway. One whose
  hand-over is refused (a first run as administrator, which UIPI keeps it
  from) says so in the log and exits.
- **A clipboard held by another program** is tried ten times, 20 ms apart,
  then reported.
- **G-SYNC stays on for Luna** where the NVIDIA driver refuses the profile
  change (administrator rights, or a profile that has the exe's name, as
  NVIDIA's "Luna" has `luna.exe`, and the person has not said Turn off) or a
  call fails: one line in the log
  naming the call and its status, one
  notice in the window pointing to the Fixed Refresh setting; Luna runs on,
  and its frames through input (vendor/PATCHES.md, patch 4) keep the refresh
  up while it is used. A record that could not be written before the save
  stops the save, so no profile is made that the undo would not know of; one
  not confirmed after it is settled at the next start. An undo the driver
  refuses leaves the profile and the record, said in the uninstaller's log
  and message; without the exe the profile changes nothing.
- **An update whose download or checks fail** writes nothing; one whose
  staged file changed, or whose installer does not start (`ShellExecuteExW`
  fails), returns the error, and the window carries on.
- **The restart after an update, where it would run elevated or that cannot
  be told** (the desktop's shell elevated, or missing, and the setup itself
  elevated; a token that does not read): Luna is not started again rather
  than started elevated; the setup's details say so.
- **Restart arguments too long for the setup's command line**: the updater
  passes none (said in the log), and the setup restarts Luna without any
  where its own command line filled NSIS's string (said in its details).
