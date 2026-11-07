#Requires -Version 7.4
# Builds the Windows exe + NSIS installer inside Docker (no local Rust needed).
# Results:
#   src-tauri\target\x86_64-pc-windows-msvc\release\luna.exe                   (the bare exe)
#   src-tauri\target\x86_64-pc-windows-msvc\release\core-fingerprint.txt       (build.rs, for latest.json)
#   src-tauri\target\x86_64-pc-windows-msvc\release\bundle\nsis\*-setup.exe    (installer + .sig)
# The NSIS step downloads a helper DLL from GitHub on every run, and a blip
# there would otherwise take the bare exe down with it. Pass -Installer
# when you actually need the setup + updater artifacts.
#
#   ./scripts/build-windows.ps1                                 the bare exe, unsigned
#   ./scripts/build-windows.ps1 -Installer -UpdaterKey <path>   the signed setup
#   ./scripts/build-windows.ps1 -Installer -Unsigned            the setup without its .sig
#
# The installer comes with the updater's .sig, so -Installer needs the updater's
# private key: the file -UpdaterKey or LUNA_UPDATER_KEY names, or else the key
# itself in TAURI_SIGNING_PRIVATE_KEY, as the release workflow has it from its
# secrets. TAURI_SIGNING_PRIVATE_KEY_PASSWORD is passed on when set; the key
# kept on the maintainer's machine has none. -Unsigned builds the setup without
# a key, for a rehearsal of a release (a dry run of the release workflow off a
# tag). The bare exe is never signed and needs no key.
param(
    [switch]$Installer,
    [switch]$Unsigned,
    [string]$UpdaterKey = $env:LUNA_UPDATER_KEY
)
. "$PSScriptRoot/lib.ps1"

if ($Unsigned -and -not $Installer) { throw '-Unsigned is for -Installer; the bare exe is never signed' }

$keyArgs = @()
$signWith = ''
$configArg = ''
if ($Installer -and $Unsigned) {
    # Tauri stops on a missing private key while the config asks for updater
    # artifacts; this override turns them off.
    $configArg = '--config src-tauri/tauri.unsigned.conf.json'
} elseif ($Installer) {
    # Passed on by name, its value from this process's environment, so it never
    # shows on a command line; empty when there is none.
    $password = if ($env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD) { @('-e', 'TAURI_SIGNING_PRIVATE_KEY_PASSWORD') } else { @('-e', 'TAURI_SIGNING_PRIVATE_KEY_PASSWORD=') }
    if ($UpdaterKey) {
        if (-not (Test-Path -PathType Leaf $UpdaterKey)) {
            throw "no updater key at $UpdaterKey (from -UpdaterKey or LUNA_UPDATER_KEY)"
        }
        $keyPath = (Resolve-Path $UpdaterKey).Path
        # Mounted rather than passed as an env value, so it never shows in
        # `docker inspect` or the process list on the host.
        $keyArgs = @('-v', "${keyPath}:/keys/updater.key:ro") + $password
        # bash splits nothing in the value of an assignment.
        $signWith = 'TAURI_SIGNING_PRIVATE_KEY=$(cat /keys/updater.key)'
    } elseif ($env:TAURI_SIGNING_PRIVATE_KEY) {
        $keyArgs = @('-e', 'TAURI_SIGNING_PRIVATE_KEY') + $password
    } else {
        throw '-Installer signs the updater artifacts and needs the private key: pass -UpdaterKey <path>, set LUNA_UPDATER_KEY or TAURI_SIGNING_PRIVATE_KEY, or build -Unsigned.'
    }
}
$bundleArg = if ($Installer) { '--bundles nsis' } else { '--no-bundle' }
# A .sig left by an earlier build would sit beside a setup it does not sign
# once an unsigned build has replaced that setup; a signed build writes its own.
$clearSigs = if ($Installer) { 'rm -f src-tauri/target/x86_64-pc-windows-msvc/release/bundle/nsis/*.sig' } else { '' }

$build = Get-BuildStamp
Write-Host "build stamp: $build"

Use-BuildLock {
    # A failed image build throws, rather than going on to build the app in
    # whatever image was left from last time.
    $image = Build-Image luna-winbuild windows-build.Dockerfile
    # The key is kept from the package install and the install scripts it could
    # run; `tauri build` needs it, for the bundle it signs.
    Invoke-WinBuild 'app build' $build $image @"
$clearSigs
env -u TAURI_SIGNING_PRIVATE_KEY -u TAURI_SIGNING_PRIVATE_KEY_PASSWORD pnpm install --frozen-lockfile
$signWith pnpm tauri build --runner cargo-xwin --target x86_64-pc-windows-msvc $bundleArg $configArg -- --locked
"@ $keyArgs
}
