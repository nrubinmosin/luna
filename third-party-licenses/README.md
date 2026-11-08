# License texts the packages leave out

`scripts/third-party-notices.ps1` writes the notices the installer ships from
the license files each crate and npm package publishes, and from what is kept
here for what they do not cover.

## Packages without license files

A few packages publish no license file, or only an SPDX identifier, though
their license asks for its text to go with a copy. Their texts are kept here,
verbatim from the project's own repository, one folder per package name:
`crates/<name>/`, `npm/<name>/`. A package's own files win over these; the
script fails on a shipped package with neither, and warns about a folder here
whose package no longer ships.

| Folder | License | Taken from |
| --- | --- | --- |
| `crates/alloc-stdlib` | BSD-3-Clause | `LICENSE` of dropbox/rust-alloc-no-stdlib at 0a81fd6928ea |
| `crates/logos` | MIT OR Apache-2.0 | `LICENSE-MIT`, `LICENSE-APACHE` of maciejhirsz/logos at fba6c4d4255b |
| `crates/selectors` | MPL-2.0 | the license's text from mozilla.org (servo/stylo, its repository, keeps none) |
| `crates/taplo` | MIT | `LICENSE` of tamasfe/taplo at 08f343be02ce |
| `crates/webview2-com`, `crates/webview2-com-sys` | MIT | `LICENSE` of wravery/webview2-rs at edc2caf88617 |
| `npm/@tauri-apps/plugin-dialog`, `-notification`, `-updater` | MIT OR Apache-2.0 | `LICENSE_MIT`, `LICENSE_APACHE-2.0` of tauri-apps/plugins-workspace at 92e47a1c0866 |

A new package without license files gets its folder here and a row in this
table, with the commit its texts were taken from.

## What the installer carries outside the packages

`components.json` lists the rest of what the installer carries: the code the
build tools write into the interface bundle, and what goes into `setup.exe` or
`luna.exe` from outside the crate and npm graphs. Each entry names its version,
license, source and the folder here with its texts (or, with `noText`, why it
has none), and `tiedTo` the package version it was checked against. The gate
fails when the lockfiles move past that version, and when the bundle it builds
holds a tool helper no entry lists, or lacks one an entry does
(`scripts/notices-lib.ps1`, `Assert-ComponentTies`, `Assert-BundledHelpers`).
Checking an entry again means reading its texts and terms at the new version,
then updating the entry and its folder.

| Folder | For | Taken from |
| --- | --- | --- |
| `tools/vite` | Vite's preload helper and `__vite__mapDeps` | the core license in `packages/vite/LICENSE.md` of vitejs/vite at v7.3.7 |
| `tools/vite-modulepreload-polyfill` | Vite's module preload polyfill | the notice in `packages/vite/src/node/plugins/modulePreloadPolyfill.ts` of vitejs/vite at v7.3.7, verbatim (it ends mid-sentence there) |
| `tools/rollup-plugin-commonjs` | the CommonJS interop helper | the `@rollup/plugin-commonjs` section of `packages/vite/LICENSE.md` of vitejs/vite at v7.3.7 |
| `installer/nsis` | NSIS 3.11, the installer's stub and plug-ins | `COPYING` of kichik/nsis at v311 (c4f43c5a7fe2) |
| `installer/nsis-tauri-utils` | `nsis_tauri_utils.dll` 0.5.3, which Tauri's bundler embeds | `LICENSE_MIT`, `LICENSE_APACHE-2.0` of tauri-apps/nsis-tauri-utils at nsis_tauri_utils-v0.5.3 (13d9edd27b69) |
| `installer/webview2-loader` | the WebView2 loader linked into `luna.exe` | `LICENSE.txt`, `NOTICE.txt` of the Microsoft.Web.WebView2 1.0.3800.47 NuGet package, whose `WebView2LoaderStatic.lib` webview2-com-sys 0.39.1 ships byte for byte |

The Microsoft Visual C++ runtime, linked statically, has no folder: its terms
are Microsoft's Visual Studio license terms, linked from its entry.

Not shipped, so not listed: the WebView2 runtime, which the installer downloads
from Microsoft (its Evergreen bootstrapper) only when Windows lacks it, and the
Universal CRT, which is part of Windows.
