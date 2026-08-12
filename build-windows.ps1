# Builds the Windows portable exe + NSIS installer inside Docker (no local Rust needed).
# Results:
#   src-tauri\target\x86_64-pc-windows-msvc\release\llm-desktop.exe            (portable)
#   src-tauri\target\x86_64-pc-windows-msvc\release\bundle\nsis\*-setup.exe    (installer + .sig)
$ErrorActionPreference = 'Stop'

$updaterKey = "C:\Users\Nikita\claude-accounts\llm-desktop-updater.key"

docker build -t llm-desktop-winbuild -f docker/windows-build.Dockerfile docker

docker run --rm `
  -e COREPACK_ENABLE_DOWNLOAD_PROMPT=0 `
  -e TAURI_SIGNING_PRIVATE_KEY_PASSWORD= `
  -v "${updaterKey}:/keys/updater.key:ro" `
  -v "${PWD}:/app" `
  -v llm-desktop-node-modules:/app/node_modules `
  -v llm-desktop-xwin-cache:/xwin-cache `
  -v llm-desktop-cargo-registry:/usr/local/cargo/registry `
  -w /app `
  llm-desktop-winbuild `
  bash -c 'export TAURI_SIGNING_PRIVATE_KEY="$(cat /keys/updater.key)" && pnpm install --frozen-lockfile && pnpm tauri build --runner cargo-xwin --target x86_64-pc-windows-msvc --bundles nsis'
