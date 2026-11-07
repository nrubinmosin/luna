# Builds the Windows exe + NSIS installer inside Docker (no local Rust needed).
# Results:
#   src-tauri\target\x86_64-pc-windows-msvc\release\luna.exe                   (the bare exe)
#   src-tauri\target\x86_64-pc-windows-msvc\release\bundle\nsis\*-setup.exe    (installer + .sig)
# The NSIS step downloads a helper DLL from GitHub on every run, and a blip
# there would otherwise take the bare exe down with it. Pass -Installer
# when you actually need the setup + updater artifacts.
#
#   ./scripts/build-windows.ps1                                 the bare exe, unsigned
#   ./scripts/build-windows.ps1 -Installer -UpdaterKey <path>   the signed setup
#
# The installer comes with the updater's .sig, so -Installer needs the updater's
# private key: -UpdaterKey, or the path in LUNA_UPDATER_KEY. The bare exe is never
# signed and needs no key.
param(
    [switch]$Installer,
    [string]$UpdaterKey = $env:LUNA_UPDATER_KEY
)
. "$PSScriptRoot/lib.ps1"

$keyArgs = @()
$signStep = ''
if ($Installer) {
    if (-not $UpdaterKey) {
        throw '-Installer signs the updater artifacts and needs the private key: pass -UpdaterKey <path> or set LUNA_UPDATER_KEY.'
    }
    if (-not (Test-Path -PathType Leaf $UpdaterKey)) {
        throw "no updater key at $UpdaterKey (from -UpdaterKey or LUNA_UPDATER_KEY)"
    }
    $keyPath = (Resolve-Path $UpdaterKey).Path
    # The key is passwordless. Mounted rather than passed as an env value, so it
    # never shows in `docker inspect` or the process list on the host.
    $keyArgs = @(
        '-e', 'TAURI_SIGNING_PRIVATE_KEY_PASSWORD=',
        '-v', "${keyPath}:/keys/updater.key:ro"
    )
    # bash splits nothing in the value of an `export`.
    $signStep = 'export TAURI_SIGNING_PRIVATE_KEY=$(cat /keys/updater.key)'
}
$bundleArg = if ($Installer) { '--bundles nsis' } else { '--no-bundle' }

$build = Get-BuildStamp
Write-Host "build stamp: $build"

# A failed image build would otherwise go on to build the app in whatever image
# was left from last time.
Build-Image luna-winbuild windows-build.Dockerfile

$cmd = ConvertTo-BashScript @"
set -e
$signStep
pnpm install --frozen-lockfile
pnpm tauri build --runner cargo-xwin --target x86_64-pc-windows-msvc $bundleArg
"@

Use-BuildLock {
    Invoke-Checked 'app build' {
        docker run --rm `
          -e COREPACK_ENABLE_DOWNLOAD_PROMPT=0 `
          -e LUNA_BUILD=$build `
          @keyArgs `
          -v "${RepoRoot}:/app" `
          @WinBuildVolumes `
          -w /app `
          luna-winbuild `
          bash -c $cmd
    }
}
