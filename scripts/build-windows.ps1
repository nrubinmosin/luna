# Builds the Windows exe + NSIS installer inside Docker (no local Rust needed).
# Results:
#   src-tauri\target\x86_64-pc-windows-msvc\release\luna.exe                   (the bare exe)
#   src-tauri\target\x86_64-pc-windows-msvc\release\bundle\nsis\*-setup.exe    (installer + .sig)
# The NSIS step downloads a helper DLL from GitHub on every run, and a blip
# there would otherwise take the bare exe down with it. Pass -Installer
# when you actually need the setup + updater artifacts.
param([switch]$Installer)
. "$PSScriptRoot/lib.ps1"

$bundleArg = if ($Installer) { '--bundles nsis' } else { '--no-bundle' }

$updaterKey = "C:\Users\Nikita\.ssh\luna-updater.key"

$build = Get-BuildStamp
Write-Host "build stamp: $build"

# A failed image build would otherwise go on to build the app in whatever image
# was left from last time.
Build-Image luna-winbuild windows-build.Dockerfile

# bash splits nothing in the value of an `export`.
$cmd = ConvertTo-BashScript @"
set -e
export TAURI_SIGNING_PRIVATE_KEY=`$(cat /keys/updater.key)
pnpm install --frozen-lockfile
pnpm tauri build --runner cargo-xwin --target x86_64-pc-windows-msvc $bundleArg
"@

Use-BuildLock {
    Invoke-Checked 'app build' {
        docker run --rm `
          -e COREPACK_ENABLE_DOWNLOAD_PROMPT=0 `
          -e LUNA_BUILD=$build `
          -e TAURI_SIGNING_PRIVATE_KEY_PASSWORD= `
          -v "${updaterKey}:/keys/updater.key:ro" `
          -v "${RepoRoot}:/app" `
          @WinBuildVolumes `
          -w /app `
          luna-winbuild `
          bash -c $cmd
    }
}
