# Builds the Windows portable exe + NSIS installer inside Docker (no local Rust needed).
# Results:
#   src-tauri\target\x86_64-pc-windows-msvc\release\llm-desktop.exe            (portable)
#   src-tauri\target\x86_64-pc-windows-msvc\release\bundle\nsis\*-setup.exe    (installer + .sig)
# The NSIS step downloads a helper DLL from GitHub on every run, and a blip
# there would otherwise take the portable exes down with it. Pass -Installer
# when you actually need the setup + updater artifacts.
param([switch]$Installer)
$ErrorActionPreference = 'Stop'

$bundleArg = if ($Installer) { '--bundles nsis' } else { '--no-bundle' }

$updaterKey = "C:\Users\Nikita\claude-accounts\llm-desktop-updater.key"

docker build -t llm-desktop-winbuild -f docker/windows-build.Dockerfile docker

$cmd = @"
set -e
export TAURI_SIGNING_PRIVATE_KEY="`$(cat /keys/updater.key)"
pnpm install --frozen-lockfile
pnpm tauri build --runner cargo-xwin --target x86_64-pc-windows-msvc $bundleArg
"@ -replace "`r`n", "`n"

docker run --rm `
  -e COREPACK_ENABLE_DOWNLOAD_PROMPT=0 `
  -e TAURI_SIGNING_PRIVATE_KEY_PASSWORD= `
  -v "${updaterKey}:/keys/updater.key:ro" `
  -v "${PWD}:/app" `
  -v llm-desktop-node-modules:/app/node_modules `
  -v llm-desktop-xwin-cache:/xwin-cache `
  -v llm-desktop-cargo-registry:/usr/local/cargo/registry `
  -v llm-desktop-tauri-cache:/root/.cache/tauri `
  -w /app `
  llm-desktop-winbuild `
  bash -c $cmd
