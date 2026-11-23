#Requires -Version 7.4
# Builds the Windows exe + NSIS installer inside Docker (no local Rust needed).
# Results:
#   src-tauri\target\x86_64-pc-windows-msvc\release\luna.exe                   (the bare exe)
#   src-tauri\target\x86_64-pc-windows-msvc\release\core-fingerprint.txt       (build.rs, for latest.json)
#   src-tauri\target\x86_64-pc-windows-msvc\release\bundle\nsis\*-setup.exe    (the installer)
#   src-tauri\target\THIRD-PARTY-NOTICES.txt                                  (with -Installer: what the setup installs beside the exe)
#
#   ./scripts/build-windows.ps1                 the bare exe
#   ./scripts/build-windows.ps1 -Installer      and the setup, made by makensis from
#                                               src-tauri/windows/installer.nsi
#                                               (docs/development.md, "Installer")
#   ... -Source <tree>                          of another tree than this checkout: an
#                                               export of its HEAD (release.ps1 -Local), so
#                                               the build can write nothing the signing runs
#
# The build stamp is always this checkout's: a -Source tree is to hold the
# commit it names.
#
# Nothing is signed here, and no key is taken: the build runs every
# dependency's build scripts and proc-macros, any of which could read a key
# in reach. scripts/release-sign.ps1 makes the setup's .sig afterwards, in a
# container that runs nothing of the build's.
param(
    [switch]$Installer,
    [string]$Source
)
. "$PSScriptRoot/lib.ps1"
$Source = if ($Source) { [IO.Path]::GetFullPath($Source, (Get-Location).Path) } else { $RepoRoot }

$release = 'src-tauri/target/x86_64-pc-windows-msvc/release'

# The installer: makensis over the exe just built, into the folder and under
# the name the release scripts find the setup by. -WX: a warning fails the
# build, as hooks.nsh's own unresolved names do. The setup itself is an x86
# program, as every NSIS setup is, whatever it installs (the updater checks
# for one, desktop/updater.rs `is_nsis_setup`); what it installs is held to
# an AMD64 image here first, its PE header's machine read at the offset the
# DOS header gives. A .sig an older build left would sit beside a setup it
# does not sign.
$makensis = ''
if ($Installer) {
    $manifest = Get-Content (Join-Path $Source 'src-tauri/Cargo.toml') -Raw
    # x.y.z, or x.y.z-pre as release-lib.ps1 Assert-VersionFormat allows; Windows'
    # version resource takes the numbers alone (VIVERSION).
    if ($manifest -notmatch '(?m)^version\s*=\s*"((\d+\.\d+\.\d+)(-[0-9A-Za-z.-]+)?)"') { throw 'src-tauri/Cargo.toml has no x.y.z version for the installer' }
    $version = $Matches[1]
    $viVersion = "$($Matches[2]).0"
    $makensis = @"
rm -f $release/bundle/nsis/*.sig
pe=`$(od -An -tu4 -j60 -N4 $release/luna.exe | tr -d ' ')
machine=`$(od -An -tx2 -j`$((pe + 4)) -N2 $release/luna.exe | tr -d ' ')
test `$machine = 8664 || { echo luna.exe is not an AMD64 image, its PE machine is 0x`$machine >&2; exit 1; }
mkdir -p $release/bundle/nsis
makensis -V3 -WX -DVERSION=$version -DVIVERSION=$viVersion -DMAINBINARYSRCPATH=/app/$release/luna.exe -DNOTICES=/app/src-tauri/target/THIRD-PARTY-NOTICES.txt -DINSTALLERICON=/app/src-tauri/icons/icon.ico -DOUTFILE=/app/$release/bundle/nsis/rm-luna_${version}_x64-setup.exe src-tauri/windows/installer.nsi
"@
}

$build = Get-BuildStamp
Write-Host "build stamp: $build"

# Made from the built tree's lockfiles before the build, outside the build lock,
# which the script takes for itself.
if ($Installer) {
    Initialize-HostDirs 'src-tauri/target' -Root $Source
    & "$PSScriptRoot/third-party-notices.ps1" -Source $Source -Out (Join-Path $Source 'src-tauri/target/THIRD-PARTY-NOTICES.txt')
}

Use-BuildLock {
    # A failed image build throws, rather than going on to build the app in
    # whatever image was left from last time.
    $image = Build-Image luna-winbuild windows-build.Dockerfile
    Invoke-WinBuild 'app build' $build $image @"
cd src-tauri
cargo xwin build --locked --release --target x86_64-pc-windows-msvc
cd ..
$makensis
"@ -Root $Source
}
