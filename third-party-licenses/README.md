# License texts the packages leave out

`scripts/third-party-notices.ps1` writes the notices the installer ships from
the license files each crate and npm package publishes. A few publish none, or
only an SPDX identifier, though their license asks for its text to go with a
copy. Their texts are kept here, verbatim from the project's own repository,
one folder per package name: `crates/<name>/`, `npm/<name>/`. A package's own
files win over these; the script fails on a shipped package with neither, and
warns about a folder here whose package no longer ships.

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
