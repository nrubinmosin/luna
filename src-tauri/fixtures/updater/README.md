# Updater fixtures

A signature made by Tauri's own signer, for `desktop/updater.rs`'s test that
Luna's updater checks what the release's signer makes (prehashed minisign,
the version in the trusted comment).

- `key.pub`: the public half of a throwaway key, as `tauri signer generate`
  writes it (base64 of a minisign public key file, the form
  `tauri.conf.json` holds Luna's in). The private half was deleted once the
  signature was made; nothing else was ever signed with it.
- `setup.bin`: a stand-in for an installer.
- `setup.bin.sig`: its signature for version 0.20.0.

Made with `scripts/updater-signature.mjs` and the Tauri CLI that
`pnpm-lock.yaml` pins (2.12.5), in the Node image, from the folder holding
`setup.bin`:

```sh
cli=node_modules/.pnpm/@tauri-apps+cli-linux-x64-gnu@2.12.5/node_modules/@tauri-apps/cli-linux-x64-gnu/cli.linux-x64-gnu.node
node scripts/updater-signature.mjs generate $cli key
TAURI_SIGNING_PRIVATE_KEY_PATH=key TAURI_SIGNING_PRIVATE_KEY_PASSWORD= \
  node scripts/updater-signature.mjs sign $cli setup.bin 0.20.0
node scripts/updater-signature.mjs verify setup.bin setup.bin.sig "$(cat key.pub)" 0.20.0
rm key
```

## Setup headers

The start of two real NSIS setups, for `desktop/updater.rs`'s test that the
updater takes an NSIS setup for one (`is_nsis_setup`): each file is the
setup's first 1024 bytes (its DOS and PE headers and section table), then the
28 bytes of NSIS's first header, read where the sections' data ends. The test
lays them out again with zeros for the rest.

- `setup-0.19.0.head`: `Luna_0.19.0_x64-setup.exe` as released (Tauri's
  bundler, NSIS 3.11), sha256
  `3438350EF2312A844288029E46260BBFB0EC3B2365896A8D9C5A310D129A01C9`.
- `setup-native.head`: the setup `scripts/build-windows.ps1 -Installer
  -Native` made of `windows/installer.nsi` (makensis 3.11).

Taken with PowerShell, `$end` being where the sections' data ends (the
largest PointerToRawData + SizeOfRawData in the section table):

```powershell
$fs = [IO.File]::OpenRead($setup); $b = [byte[]]::new(1052)
[void]$fs.Read($b, 0, 1024); $fs.Position = $end; [void]$fs.Read($b, 1024, 28); $fs.Close()
[IO.File]::WriteAllBytes($out, $b)
```
