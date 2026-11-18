# Desktop services

What Tauri's plugins and tray do for the window, as plain Rust in
`src-tauri/src/desktop/` that needs no Tauri: for the native (GPUI) window of
release 0.20 ([ADR-0030](../adr/0030-scope-of-0-20.md)). Each service is
functions and structs taking callbacks, no window type of any toolkit, so the
window wires one in with a few lines. None is wired into `run()` yet: the
Tauri window there still uses the plugins.

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
| `desktop/single.rs` | `claim(on_second) -> Claim` | tauri-plugin-single-instance |
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
- **Notifications**: each is shown from a thread of its own, which then waits
  for its answer; `on_click`'s callback runs there. macOS delivers the answer
  on the main run loop, which GPUI runs.
- **Single instance**: the listener has its own thread (a message-only
  window and its message loop on Windows, a socket's accept loop elsewhere);
  the callback runs there.
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
shape. On Windows, on `notify-rust`, a toast under the AUMID the installer
gives the Start-menu shortcut (`rm.luna`); run from a cargo target folder,
which has no such shortcut, under PowerShell's, as the plugin does. A click on
the toast's body calls `on_click` with the chat id, and the window brings
itself up and opens that chat; its buttons and its closing are no click. A
click reaches the running Luna only: one left in the notification center
after Luna has quit does nothing, as today. On macOS, `NSUserNotification`
under the bundle's identifier when Luna runs from `Luna.app`, which needs no
signing, only the bundle LaunchServices has seen. Linux shows it and reports
no click.

**Single instance.** One window per identifier and data dir, which is one
core's: the test build runs beside the Luna in use, and so does a copy of
Luna in another folder. Windows: a mutex `Local\<identifier>-<hash of the data
dir>-sim` says who is first; the first keeps a message-only window (class
`…-sic`, title `…-siw`) that takes a second launch's arguments by
WM_COPYDATA, tagged `LUNA`, at most 64 KB, and the second lets it take the
foreground (`AllowSetForegroundWindow`) before it exits. Unix: an exclusive
`flock` on `<data>/luna.window.lock`, and the first listens on
`<data>/luna.window.sock` (0600). `claim` makes no claim under
`LUNA_DEV_TOKEN`; in the background (`crate::background`) it claims, but a
second launch does not call the window up, as today. The second launch's
`Claim::Second` tells the caller to exit; the plugin exited for it.

**Dialogs.** On `rfd`, as the plugin. A missing start folder opens at its
nearest folder that is there; extensions are taken as `md`, `.md` or `*.md`.

**Clipboard.** `read` returns what a paste would put in a terminal, the first
of: files (CF_HDROP; Finder's file URLs, file reference URLs resolved through
NSURL), text, an image as PNG with its size (the "PNG" format a browser puts
there, else a CF_DIBV5 or CF_DIB of 24 or 32 bits, uncompressed or by bit
fields, made PNG; on macOS PNG or TIFF made PNG by AppKit). Text comes before
an image because a spreadsheet's cells come with a picture of themselves. The
window types files as quoted paths and, for an image, presses the CLI's image
key (ADR-0030). The Windows formats are parsed by functions the tests hold to
hand-made bytes; no test reads or writes the real clipboard, which is the
person's.

**Window state.** `<data>/window-state.json` holds the window's normal
bounds and whether it is maximized, in the coordinates the toolkit gives its
displays in. `fit` holds them to the displays there are now: the window lands
on the display it overlaps most, or centered on the primary when it overlaps
none, no smaller than the minimum and no larger than that display, and
wholly on it. Nothing is saved or restored in the background. What 0.19's
plugin saved under the identifier is not read.

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
- **Download into the data dir**: the installer goes to `<data>/updates/`
  under the URL's own name when it is a plain one, through `write_private`,
  only after its signature checked out and, on Windows, it starts as an
  executable (`MZ`). The plugin wrote it to the system's temp folder.
  `clear_downloads` empties the folder, for the window to call as it starts.
- **The same start**: `install` checks the file's signature again and runs
  it with `ShellExecuteW` "open", `/P /UPDATE /R /ARGS <this run's
  arguments>`, each argument quoted Windows' way and a `/` quoted too (the
  plugin's escaping); the caller then exits at once, as the plugin's
  `exit(0)` did. `/P` is passive (a progress bar only), `/UPDATE` installs
  in place without uninstalling first or touching the shortcuts, `/R` starts
  Luna again, `/ARGS` with those arguments. Off Windows an update is
  installed by hand.

The page's part stays the window's: the chip, the notes, the core
fingerprint compared to decide whether the sessions keep running, the stop of
the sessions before the install (power-and-lifecycle.md, "Updates").

## Installer

`windows/installer.nsi`, built with makensis (NSIS 3.11, in the Windows build
image) by `scripts/build-windows.ps1 -Installer -Native`, into the folder and
under the name Tauri's bundler gives the setup
(`…/release/bundle/nsis/Luna_<version>_x64-setup.exe`). It includes
`windows/hooks.nsh` unchanged and calls its hooks where Tauri's template does:
the install folder's DACL before the first file, the core's stop and the
`core\` copies' removal before uninstalling, the WebView profile's removal
when the app data is deleted.

Compared with Tauri's template (tauri-bundler 2.12.5, read, not copied):

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
| Helper DLL | `nsis_tauri_utils` (semver compare, de-elevated start, string replace), downloaded on every build | none; Luna starts as the user running the setup, which a per-user install runs as |

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
- The third-party notices: NSIS still, its Tauri plug-in and the WebView2
  loader no more (`third-party-licenses/components.json`).
- A macOS job: build the exe on a macOS runner, `scripts/macos-app.sh`, and
  publish the bundle (zipped) as an experimental, unsigned asset; the
  updater offers macOS nothing until `latest.json` names a `darwin-*`
  platform.

## Failure modes

- **Toasts do not show** when the exe runs without a shortcut carrying its
  AUMID (a copy run from another folder than the installer's, the test
  build): Windows drops a toast of an unknown AUMID. As with the plugin.
- **The exe needs a manifest for common controls v6**: `tray-icon`'s menus
  (muda, with the common-controls-v6 feature Tauri turns on in this tree)
  import comctl32 functions that only load with it, as lib.rs's `acl` tests
  note. Tauri's build embeds that manifest today; the native window's build
  must too. The unit tests never make a tray for that reason.
- **A second launch that finds no listener** (the first still starting, or
  hung) waits a second, says so in the log and exits anyway.
- **A clipboard held by another program** is tried ten times, 20 ms apart,
  then reported.
- **An update whose download or signature fails** writes nothing; one whose
  installer does not start (`ShellExecuteW` at 32 or below) returns the
  error, and the window carries on.
