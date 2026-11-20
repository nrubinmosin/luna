# Desktop services

What Tauri's plugins and tray do for the window, as plain Rust in
`src-tauri/src/desktop/` that needs no Tauri: for the native (GPUI) window of
release 0.20 ([ADR-0030](../adr/0030-scope-of-0-20.md)). Each service is
functions and structs taking callbacks, no window type of any toolkit, so the
window wires one in with a few lines. The native window wires them all in
(native/desktop.rs; [native-window.md](native-window.md), "Desktop
services"); the single instance is claimed in lib.rs `run` for both kinds of
window, and the WebView window keeps Tauri's plugins for the rest.

## Responsibility

- Do for the native window what the Tauri window gets from its plugins: the
  tray, notifications, a single instance, file pickers, the window's size and
  place, the updater; and the clipboard, which the page had from the WebView.
- Stay compatible with what 0.19 installed: an update from 0.19 lands on a
  0.20 that its updater, installer and shortcut recognize, and 0.20 updates
  onto later releases the same way.
- Keep everything in the data dir (`paths::data_dir`), as Luna keeps the rest.

## Key modules

| Path | What it holds | Replaces |
| --- | --- | --- |
| `desktop/mod.rs` | `identifier()`, `product_name()`: `rm.luna`/`Luna`, the test build's `rm.luna.test`/`Luna Test` | `tauri.conf.json` |
| `desktop/tray.rs` | `Tray::new(Actions)`, `Tray::set_awake`; the icon, its dot, the tooltip | `TrayIconBuilder` in `run()`, `tray_awake` |
| `desktop/notify.rs` | `show(Notice)`, `on_click(f)` | tauri-plugin-notification, the page's `notify()` |
| `desktop/single.rs` | `claim(on_second) -> Claim`, focus only; `desktop::bring_up` what it calls | tauri-plugin-single-instance (both windows) |
| `desktop/dialog.rs` | `folder`, `file` and their `_async` forms, `Ask` | tauri-plugin-dialog, the page's `pickFolder` |
| `desktop/clipboard.rs` | `read() -> Clip`, `write_text` | the WebView's clipboard |
| `desktop/window_state.rs` | `load`, `save`, `fit` | tauri-plugin-window-state |
| `desktop/updater.rs` | `check`, `download`, `install`, `verify` | tauri-plugin-updater, `updates.store.ts` |
| `windows/installer.nsi` | the NSIS installer made without Tauri's bundler | Tauri's NSIS template |
| `scripts/macos-app.sh` | `Luna.app`, unsigned, for CI's macOS runner | Tauri's macOS bundler |

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

**Notifications.** `Notice { title, body, chat_id, sound }`, the page's call
shape. On Windows, on `tauri-winrt-notification` (the crate under
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

## Updater

Compatible with tauri-plugin-updater both ways, so a 0.19 window updates onto
0.20 and 0.20 onto later releases:

- **The same endpoint and key**: `ENDPOINT` and `PUBKEY`, held by a test to
  `tauri.conf.json`'s `plugins.updater` while that file is there.
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
- **Stricter than the plugin**: only an `https://` URL; nothing older than
  or the same as the running build is downloaded or installed, whoever asks
  (`parse` offers none, and `download` and `install` refuse one too); the
  setup must be an NSIS setup as well as signed (`is_nsis_setup`): its stub
  a 32-bit x86 GUI executable (`MZ`, `PE\0\0`, machine 0x014C, PE32,
  subsystem 2, no DLL), as every NSIS setup's is whatever it installs (0.19's
  and the native one alike), and right where its sections' data ends NSIS's
  first header (`0xDEADBEEF`, `NullsoftInst`) with as many bytes after it as
  it says. That what the setup installs is AMD64 is held at build time:
  `build-windows.ps1 -Native` refuses a `luna.exe` whose PE machine is not
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
  folder. `clear_downloads` empties the folder, for the window to call as it
  starts.
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

The page's part stays the window's: the chip, the notes, the core
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
image) by `scripts/build-windows.ps1 -Installer -Native`, into the folder and
under the name Tauri's bundler gives the setup
(`…/release/bundle/nsis/Luna_<version>_x64-setup.exe`). It includes
`windows/hooks.nsh` unchanged and calls its hooks where Tauri's template does:
the install folder's DACL before the first file, the core's stop and the
`core\` copies' removal before uninstalling, the WebView profile's removal
when the app data is deleted. A pre-release version (`x.y.z-pre`, as
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

What `release.yml` and the release scripts will need once the native window
ships (nothing of it is changed yet):

- The build job: `build-windows.ps1 -Installer -Native`, once the exe is the
  native window's. The setup's name and folder are the same, so
  `release-bundle.ps1`, `release-sign.ps1` and `latest.json` take it as they
  are.
- `release-lib.ps1` reads the endpoint (`Get-UpdaterEndpoint`) and
  `release-sign.ps1`/`updater-signature.mjs` the public key from
  `tauri.conf.json`, and `release.ps1` writes the version there: when
  `tauri.conf.json` goes, they read `desktop/updater.rs`'s `ENDPOINT` and
  `PUBKEY` (or a file both read) and the version files lose it.
- `release-sign.ps1` signs with the Tauri CLI's `signer sign` from
  `pnpm-lock.yaml`; with no JS left in the repository it needs the CLI
  pinned some other way, or a minisign signer of its own that writes the
  same form (prehashed, `version:` in the trusted comment).
- The third-party notices: NSIS and Tauri's template still, its Tauri
  plug-in and the WebView2 loader no more
  (`third-party-licenses/components.json`).
- A macOS job: build the exe on a macOS runner, `scripts/macos-app.sh`, and
  publish the bundle (zipped) as an experimental, unsigned asset; the
  updater offers macOS nothing until `latest.json` names a `darwin-*`
  platform.

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
