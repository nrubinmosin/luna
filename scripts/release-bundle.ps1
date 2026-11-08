#Requires -Version 7.4
# Gathers what a release publishes from the last `./scripts/build-windows.ps1
# -Installer` into one folder: the installer and the latest.json the updater
# reads, its signature still empty. The release workflow
# (.github/workflows/release.yml) and `./scripts/release.ps1 -Local` both make
# their bundle here, so the manifest is written one way, and both then sign it
# with scripts/release-sign.ps1, which writes the .sig and puts it into
# latest.json.
#
#   ./scripts/release-bundle.ps1 -Out <dir>                 for the version in the version files
#   ./scripts/release-bundle.ps1 -Out <dir> -Notes "..."    the notes latest.json carries (the
#                                                           CHANGELOG section's intro); "Luna <version>" without
#   ./scripts/release-bundle.ps1 -Out <dir> -Source <tree>  from the build of the tree
#                                                           `build-windows.ps1 -Source` built
#
# -Out has to be empty or not there yet: nothing in it is overwritten or deleted.
# What the build left is only read, never run, and never through a link.
param(
    [Parameter(Mandatory)][string]$Out,
    [string]$Notes,
    [string]$Source
)
. "$PSScriptRoot/lib.ps1"
. "$PSScriptRoot/release-lib.ps1"

$version = Get-ReleaseVersion
$built = if ($Source) {
    $Source = [IO.Path]::GetFullPath($Source, (Get-Location).Path)
    Find-ReleaseArtifacts $version -Dir (Join-Path $Source 'src-tauri/target/x86_64-pc-windows-msvc/release') -Root $Source
} else {
    Find-ReleaseArtifacts $version
}

# The core fingerprint build.rs left beside the exe, the value it stamps the
# exe with; coreproc.rs holds the exe's `--core-fingerprint` to it in a test.
# The exe itself is not asked: that would run the build's output here.
$fingerprint = (Get-Content -LiteralPath $built.Fingerprint -Raw).Trim()
Assert-CoreFingerprint $fingerprint $built.Fingerprint

$Out = [IO.Path]::GetFullPath($Out, (Get-Location).Path)
if (Test-Path -LiteralPath $Out) {
    if (Get-ChildItem -LiteralPath $Out -Force) { throw "$Out is not empty" }
} else {
    New-Item -ItemType Directory -Force $Out | Out-Null
}

$setupName = Split-Path -Leaf $built.Setup
Copy-Item -LiteralPath $built.Setup -Destination $Out

# No installed Luna takes an empty signature: unsigned, the bundle is a
# rehearsal (a dry run off a branch) until release-sign.ps1 signs it.
$manifest = New-UpdaterManifest -Version $version -CoreFingerprint $fingerprint -Signature '' `
    -SetupName $setupName -Endpoint (Get-UpdaterEndpoint) -Notes $Notes
$manifest | Set-Content (Join-Path $Out 'latest.json') -Encoding utf8

Write-Host "release bundle for v$version (core $fingerprint) -> $Out"
Get-ChildItem -LiteralPath $Out | ForEach-Object { Write-Host "  $($_.Name)" }
