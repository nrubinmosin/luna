# Luna

*[Русская версия](README.ru.md)*

A thin desktop shell (Rust + Tauri) around the Claude Code CLI: several sessions side by
side in panes, project folders, and accounts with isolated configs. It drives the
**globally installed** `claude` binary.

For how it is put together, see [ARCHITECTURE.md](ARCHITECTURE.md).

## Features

- A Windows XP shell (xp.css, Luna Blue): the native window frame is replaced by our own
  title bar with minimize / maximize / close, and the UI runs on Tahoma. The whole
  interface scales from one variable, `--ui` in `src/app/theme.css`.
- A grid of 1/2/3/4 panes, with chats dragged in from the sidebar (⌘/Ctrl+1..4).
- Window groups I/II/III/IV — tabs under the layout selector. Each group remembers its
  own four boards (one per layout) and their splits, and has its own reset button (↺ on
  the tab) that clears only the arrangement and leaves every chat alone.
- New chat (⌘/Ctrl+N): folder, model, effort (`low/medium/high/xhigh/max`, defaults to
  `medium`), permission mode, account, and a **Git worktree** checkbox (on by default →
  `claude --worktree`).
- Accounts: a panel in the status bar. "Add" creates `Documents/claude-accounts/<name>`,
  "✕" deletes the folder. A chat's session starts with
  `CLAUDE_CONFIG_DIR=<account folder>`.
- Chat marks: a ★ at the head of the row, set and cleared by clicking it. It means
  whatever you decide it means — nothing in the app reads it. On unmarked rows it only
  shows under the cursor.
- Live account limits (5h / week / model-weekly, plus reset times) — taken from the OAuth
  usage endpoint, so they cost no tokens; refreshed once a minute.
- Closing the window hides the app to the tray and leaves the sessions running. Quit from
  the tray menu.
- Light / dark / system themes.

## Development

```sh
pnpm install
pnpm dev          # frontend only, in a browser (tauri commands are no-ops)
pnpm tauri dev    # the whole app (needs a Rust toolchain)
```

## Checking the Rust side without a local toolchain (Docker)

```powershell
docker run --rm -v "${PWD}:/app" -w /app/src-tauri rust:1-bookworm bash -c `
  "apt-get update -qq && apt-get install -y -qq libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev libssl-dev pkg-config build-essential libxdo-dev >/dev/null && cargo check"
```

## Windows build through Docker

```powershell
./build-windows.ps1
```

Builds the toolchain image (rust + cargo-xwin + NSIS + node) and cross-compiles for
`x86_64-pc-windows-msvc`:

- the portable exe — `src-tauri\target\x86_64-pc-windows-msvc\release\luna.exe`
  (self-contained; it only needs the system WebView2);
- the NSIS installer — `...\release\bundle\nsis\Luna_*_x64-setup.exe`.

The caches (node_modules, the Windows SDK, the cargo registry) live in named volumes, so
repeat builds are fast.

## Updates and signing

- On startup the app checks `latest.json` from GitHub Releases (the endpoint lives in
  `src-tauri/tauri.conf.json` → `plugins.updater.endpoints`; replace `OWNER` with your
  own account or org once the repository exists on GitHub).
- The update signing key is `C:\Users\Nikita\claude-accounts\llm-desktop-updater.key` —
  private and passwordless, so keep it safe; its `.pub` half is already baked into the
  config. The file name is left over from the pre-Luna brand: if you rename it, fix the
  path in `build-windows.ps1` too. For a signed build, set
  `TAURI_SIGNING_PRIVATE_KEY_PATH` in the build environment — `.sig` files then appear
  next to the bundle, and those plus `latest.json` are what you upload to the Release.
- Authenticode-signing the exe (so SmartScreen stops complaining) needs a certificate.
  When there is one, add it under `tauri.conf.json` → `bundle.windows.signCommand`.
