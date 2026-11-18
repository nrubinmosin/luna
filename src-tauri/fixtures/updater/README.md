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
