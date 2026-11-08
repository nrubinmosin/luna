#Requires -Version 7.4
# Builds the Windows exe + NSIS installer inside Docker (no local Rust needed).
# Results:
#   src-tauri\target\x86_64-pc-windows-msvc\release\luna.exe                   (the bare exe)
#   src-tauri\target\x86_64-pc-windows-msvc\release\core-fingerprint.txt       (build.rs, for latest.json)
#   src-tauri\target\x86_64-pc-windows-msvc\release\bundle\nsis\*-setup.exe    (the installer)
# The NSIS step downloads a helper DLL from GitHub on every run, and a blip
# there would otherwise take the bare exe down with it. Pass -Installer
# when you actually need the setup.
#
#   ./scripts/build-windows.ps1                 the bare exe
#   ./scripts/build-windows.ps1 -Installer      and the setup
#
# Nothing is signed here, and no key is taken: `tauri build` runs every
# dependency's build scripts, proc-macros and frontend tooling, any of which
# could read a key in reach. The setup is the updater's artifact as it is
# (createUpdaterArtifacts without v1Compatible), the same bytes whether or not
# Tauri signs it; scripts/release-sign.ps1 makes its .sig afterwards, in a
# container that runs nothing of the build's.
param(
    [switch]$Installer
)
. "$PSScriptRoot/lib.ps1"

# Tauri stops on a missing private key while the config asks for updater
# artifacts; this override turns them, and so the signing, off.
$bundleArg = if ($Installer) { '--bundles nsis --config src-tauri/tauri.unsigned.conf.json' } else { '--no-bundle' }
# A .sig an older build left would sit beside a setup it does not sign.
$clearSigs = if ($Installer) { 'rm -f src-tauri/target/x86_64-pc-windows-msvc/release/bundle/nsis/*.sig' } else { '' }

$build = Get-BuildStamp
Write-Host "build stamp: $build"

Use-BuildLock {
    # A failed image build throws, rather than going on to build the app in
    # whatever image was left from last time.
    $image = Build-Image luna-winbuild windows-build.Dockerfile
    Invoke-WinBuild 'app build' $build $image @"
$clearSigs
pnpm install --frozen-lockfile
pnpm tauri build --runner cargo-xwin --target x86_64-pc-windows-msvc $bundleArg -- --locked
"@
}
