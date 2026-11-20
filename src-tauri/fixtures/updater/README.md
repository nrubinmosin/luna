# Updater fixtures

Keys and signatures for the tests that hold Luna's updater signatures to the
form Tauri's signer gave them up to 0.19, both ways: what the release's signing
(`scripts/release-lib.ps1` `New-UpdaterSignature`, OpenSSL and the release
scripts) makes is what 0.19's tauri-plugin-updater and 0.20's
`desktop/updater.rs` take, and a key the Tauri CLI made is one it signs with.
Every key here is a throwaway made for these tests: no Luna trusts one, and
nothing but these fixtures was ever signed with them.

- `setup.bin`: a stand-in for an installer.
- `key.pub`, `setup.bin.sig`: a key the Tauri CLI made, its private half
  deleted, and the signature `tauri signer sign --app-version 0.20.0` made
  over `setup.bin` with it (trusted comment
  `timestamp:…\tfile:setup.bin\tversion:0.20.0`, no core fingerprint).
- `tauri.key`, `tauri.key.pub`: a key pair as `tauri signer generate` wrote
  it with an empty password, the form the maintainer's key is in (base64 of
  a minisign secret key file, encrypted with scrypt; base64 of the public
  key file). The release-lib tests and `desktop/updater.rs`'s sign with it.
- `tauri-password.key`, `tauri-password.key.pub`: the same with the password
  `luna fixture password`.
- `release-0.20.0.openssl.sig`: what the release's signing makes with
  `tauri.key` over `setup.bin` named `Luna_0.20.0_x64-setup.exe`, for 0.20.0
  and the core `0123456789abcdef`, dated 1795158621, as a release's `.sig`.
  Its Ed25519 signatures are RFC 8032's deterministic ones, so
  `release-lib.test.ps1` makes it again byte for byte; `desktop/updater.rs`
  checks it as 0.20's updater does and as 0.19's tauri-plugin-updater 2.14.1
  does, step for step, on the minisign-verify and base64 0.19 locks
  (`as_0_19_checks`), and holds it to the form of the next one.
- `release-0.20.0.sig`: the same release signed with the same key on the
  same date by `luna-updater-signer`, the signer on the `minisign` crate
  (0.10.0) that 0.20's release used before it signed with OpenSSL, its nonces
  hedged with fresh noise. tauri-plugin-updater 2.14.1 itself, the one 0.19
  ships, took it: once, at 479ce741 of `stage6/signer`, while the app still
  had the plugin and Tauri's mock runtime (a test not kept), the real plugin,
  registered in a mock app with `tauri.key.pub` as its key, fetched a
  `latest.json` offering 0.20.0 with this signature from a loopback server,
  downloaded `setup.bin` and verified it (`Update::download`,
  `Update::verify`); offered as 0.20.1 it refused it ("The update was signed
  for version 0.20.0 but the update endpoint announced version 0.20.1").
  `as_0_19_checks` says of it what the plugin said. It also stands for what
  a signer that chose its own nonces makes: valid, and refused by the
  release's check against RFC 8032 (`release-lib.test.ps1`).

Luna 0.19 pinned `@tauri-apps/cli-linux-x64-gnu` 2.12.5 and checked its
package integrity. The fixture used that native module in a networkless
Node container, as
`scripts/updater-signature.mjs` ran it up to 0.19 (in git history):

```sh
cli=cli.linux-x64-gnu.node
node updater-signature.mjs generate $cli tauri.key          # --ci --password ''
node -e "require('./$cli').run(['signer','generate','--ci','--password','luna fixture password','--write-keys','tauri-password.key'],'tauri',e=>{if(e)throw e})"
```

`release-0.20.0.openssl.sig` was made with the release scripts, as the
release-lib tests make it again:

```powershell
. ./scripts/lib.ps1; . ./scripts/release-lib.ps1
$openssl = Get-OpenSsl -ForKey
$key = Read-UpdaterSecretKey $openssl (Get-Content src-tauri/fixtures/updater/tauri.key -Raw) ''
# setup.bin copied to <dir>/Luna_0.20.0_x64-setup.exe
(New-UpdaterSignature -OpenSsl $openssl -Key $key -Path <dir>/Luna_0.20.0_x64-setup.exe -Version 0.20.0 `
    -Fingerprint 0123456789abcdef -Work <dir> -Now { [DateTimeOffset]::FromUnixTimeSeconds(1795158621) }).Signature
```

Once, when the release's signing moved to OpenSSL, the `minisign` crate the
Tauri CLI signs with (0.10.0, in a scratch project in the pinned Rust image,
not kept) opened a key `New-UpdaterKeyPair` made, with an empty password and
refusing another, derived from it the public key written beside it, and
signed with it, which `as_0_19_checks`' steps took; and the same steps took
this signing's signatures under that key and under `tauri.key`.
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
