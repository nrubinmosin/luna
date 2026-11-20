# Updater fixtures

Keys and signatures for the tests that hold Luna's updater signatures to the
form Tauri's signer gave them up to 0.19, both ways: what the release's signer
(`crates/updater-signer`, `luna-updater-signer`) makes is what 0.19's
tauri-plugin-updater and 0.20's `desktop/updater.rs` take, a key the Tauri CLI
made is one the signer signs with, and a key the signer made is one the Tauri
CLI signs with. Every key here is a throwaway made for these tests: no Luna
trusts one, and nothing but these fixtures was ever signed with them.

- `setup.bin`: a stand-in for an installer.
- `key.pub`, `setup.bin.sig`: a key the Tauri CLI made, its private half
  deleted, and the signature `tauri signer sign --app-version 0.20.0` made
  over `setup.bin` with it (trusted comment
  `timestamp:…\tfile:setup.bin\tversion:0.20.0`, no core fingerprint).
- `tauri.key`, `tauri.key.pub`: a key pair as `tauri signer generate` wrote
  it with an empty password, the form the maintainer's key is in (base64 of
  a minisign secret key file, encrypted with scrypt; base64 of the public
  key file). The signer's tests and `desktop/updater.rs`'s sign with it.
- `tauri-password.key`, `tauri-password.key.pub`: the same with the password
  `luna fixture password`.
- `signer.key.pub`, `setup.bin.signer-key.sig`: a key `luna-updater-signer
  generate` made, its private half deleted, and the signature the Tauri CLI
  made over `setup.bin` with it for 0.20.0.
- `release-0.20.0.sig`: what `luna-updater-signer sign` made with
  `tauri.key` over `setup.bin` named `Luna_0.20.0_x64-setup.exe`, for 0.20.0
  and the core `0123456789abcdef`, as a release's `.sig`. tauri-plugin-updater
  2.14.1 itself, the one 0.19 ships, took it: once, at 479ce741 of
  `stage6/signer`, while the app still had the plugin and Tauri's mock runtime
  (a test not kept), the real plugin, registered in a mock app with
  `tauri.key.pub` as its key, fetched a `latest.json` offering 0.20.0 with
  this signature from a loopback server, downloaded `setup.bin` and verified
  it (`Update::download`, `Update::verify`); offered as 0.20.1 it refused it
  ("The update was signed for version 0.20.0 but the update endpoint announced
  version 0.20.1"). The tests here check it the same way (the signer crate's,
  0.19's check step for step; `desktop/updater.rs`; `release-lib.test.ps1`,
  with OpenSSL).

The Tauri CLI was the one `pnpm-lock.yaml` pinned before 0.20 dropped Node
(`@tauri-apps/cli-linux-x64-gnu` 2.12.5, held to its lockfile integrity),
run from its native module in the pinned Node image with no network, as
`scripts/updater-signature.mjs` ran it up to 0.19 (in git history):

```sh
cli=cli.linux-x64-gnu.node
node updater-signature.mjs generate $cli tauri.key          # --ci --password ''
node -e "require('./$cli').run(['signer','generate','--ci','--password','luna fixture password','--write-keys','tauri-password.key'],'tauri',e=>{if(e)throw e})"
luna-updater-signer generate signer.key                     # in the signer's image
TAURI_SIGNING_PRIVATE_KEY_PATH=signer.key TAURI_SIGNING_PRIVATE_KEY_PASSWORD= \
  node updater-signature.mjs sign $cli setup.bin 0.20.0     # then renamed setup.bin.signer-key.sig
rm signer.key
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
