# License texts the packages leave out

`scripts/third-party-notices.ps1` writes the notices the installer ships from
the license files each crate and npm package publishes, and from what is kept
here for what they do not cover.

## Packages without license files

A few packages publish no license file, or only an SPDX identifier, though
their license asks for its text to go with a copy. Their texts are kept here,
verbatim from the project's own repository unless its row says what was
filled in and from where, one folder per package name:
`crates/<name>/`, `npm/<name>/`. A package's own files win over these; the
script fails on a shipped package with neither, and warns about a folder here
whose package no longer ships.

| Folder | License | Taken from |
| --- | --- | --- |
| `crates/accesskit` | MIT OR Apache-2.0 | `LICENSE-MIT`, `LICENSE-APACHE`, `LICENSE.chromium` of AccessKit/accesskit at a55d3e1a18bb |
| `crates/accesskit_consumer`, `crates/accesskit_windows` | MIT OR Apache-2.0 | the same three of AccessKit/accesskit at c88605b96d04 |
| `crates/alloc-stdlib` | BSD-3-Clause | `LICENSE` of dropbox/rust-alloc-no-stdlib at 0a81fd6928ea |
| `crates/anes` | MIT OR Apache-2.0 | `LICENSE-MIT`, `LICENSE-APACHE` of qwandor/anes-rs (formerly zrzka/anes-rs) at 378ac7461eca |
| `crates/lyon`, `crates/lyon_geom`, `crates/lyon_path` | MIT OR Apache-2.0 | `LICENSE-MIT`, `LICENSE-APACHE` of nical/lyon at 56219c95c658 |
| `crates/lyon_algorithms`, `crates/lyon_tessellation` | MIT OR Apache-2.0 | the same two of nical/lyon at 994526f2766c |
| `crates/mac` | MIT/Apache-2.0 | reem/rust-mac keeps no license file; its README says "MIT/Apache-2.0, like rust itself". `LICENSE-APACHE` is that of rust-lang/rust at 1.100.0, verbatim (it names no copyright holder); `LICENSE-MIT` is the MIT text of rust-lang/rust at 1.100.0 with its copyright line made the crate's own: "Jonathan Reem", the author `Cargo.toml` names in mac 0.1.1 as published (the version Cargo.lock pins), no year being given anywhere |
| `crates/profiling` | MIT OR Apache-2.0 | `LICENSE-MIT`, `LICENSE-APACHE` of aclysma/profiling at 8271551172eb |
| `crates/pulp-wasm-simd-flag` | MIT | `LICENSE` of sarah-quinones/pulp at 5eb07fd7b68e |
| `crates/rust-i18n-support` | MIT | `LICENSE` of longbridge/rust-i18n at 5f3ea5ba0339 |
| `crates/seahash` | MIT | redox-os/seahash keeps no license file. The license's text, `text/MIT.txt` of spdx/license-list-data at v3.27.0, with its `<year> <copyright holders>` placeholder made the crate's own: "ticki, Tom Almeida", the authors `Cargo.toml` names in seahash 4.1.0 as published (the version Cargo.lock pins), no year being given anywhere |
| `crates/selectors` | MPL-2.0 | the license's text from mozilla.org (servo/stylo, its repository, keeps none) |
| `crates/svg_fmt` | MIT/Apache-2.0 | `LICENSE` of nical/rust_debug at 93f414cd572b |
| `crates/taffy` | MIT | `LICENSE` of DioxusLabs/taffy at eb6450694f33 |
| `crates/tree-sitter-json` | MIT | `LICENSE` of tree-sitter/tree-sitter-json at ee35a6ebefce |
| `crates/tree-sitter-md` | MIT | `LICENSE` of tree-sitter-grammars/tree-sitter-markdown at 41ff24a6c775 |
| `crates/zune-core` | MIT OR Apache-2.0 OR Zlib | `LICENSE.md`, `LICENSE-ZLIB` of etemesi254/zune-image at f8fbb123d5ed |
| `crates/zune-inflate` | MIT OR Apache-2.0 OR Zlib | the same two of etemesi254/zune-image at 69502ce83fdf |
| `crates/zune-jpeg` | MIT OR Apache-2.0 OR Zlib | the same two of etemesi254/zune-image at fa2c767a01d7 |
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
| `tools/vite` | Vite's preload helper and `__vite__mapDeps` | the core license in `packages/vite/LICENSE.md` of vitejs/vite at v8.3.8 (b39e6e63dcbf) |
| `tools/vite-modulepreload-polyfill` | the module preload polyfill, Rolldown's port of Vite's | `LICENSE`: the notice in `packages/vite/src/node/plugins/modulePreloadPolyfill.ts` of vitejs/vite at v7.3.7, verbatim (it ends mid-sentence there); `LICENSE-rolldown`: `LICENSE` of rolldown/rolldown at v1.2.19 (b9c75ddc8bd5) |
| `tools/rolldown-runtime` | Rolldown's runtime helpers | `LICENSE`, `THIRD-PARTY-LICENSE` of rolldown/rolldown at v1.2.19 (b9c75ddc8bd5) |
| `installer/nsis` | NSIS 3.11, the installer's stub and plug-ins | `COPYING` of kichik/nsis at v311 (c4f43c5a7fe2) |
| `installer/nsis-tauri-utils` | `nsis_tauri_utils.dll` 0.5.3, which Tauri's bundler embeds | `LICENSE_MIT`, `LICENSE_APACHE-2.0` of tauri-apps/nsis-tauri-utils at nsis_tauri_utils-v0.5.3 (13d9edd27b69) |
| `installer/tauri-nsis-template` | Tauri's NSIS template, which `src-tauri/windows/installer.nsi` is adapted from | `LICENSE-MIT`, `LICENSE-APACHE-2.0` of the tauri 2.12.6 crate (tauri-apps/tauri, the repository tauri-bundler 2.10.5 is published from) |
| `installer/webview2-loader` | the WebView2 loader linked into `luna.exe` | `LICENSE.txt`, `NOTICE.txt` of the Microsoft.Web.WebView2 1.0.3800.47 NuGet package, whose `WebView2LoaderStatic.lib` webview2-com-sys 0.39.1 ships byte for byte |
| `installer/cascadia-mono` | Cascadia Mono, the native window's font, compiled into `luna.exe` | `LICENSE` of microsoft/cascadia-code at v2407.24 (56bcca3f2c1e) |

The Microsoft Visual C++ runtime, linked statically, has no folder: its terms
are Microsoft's Visual Studio license terms, linked from its entry.

Not shipped, so not listed: the WebView2 runtime, which the installer downloads
from Microsoft (its Evergreen bootstrapper) only when Windows lacks it, and the
Universal CRT, which is part of Windows.
