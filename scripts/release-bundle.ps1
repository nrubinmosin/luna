#Requires -Version 7.4
# Gathers what a release publishes from the last `./scripts/build-windows.ps1
# -Installer` into one folder: the installer, its updater signature and the
# latest.json the updater reads. The release workflow (.github/workflows/release.yml)
# and `./scripts/release.ps1 -Local` both make their bundle here, so the
# manifest is written one way.
#
#   ./scripts/release-bundle.ps1 -Out <dir>                 for the version in the version files
#   ./scripts/release-bundle.ps1 -Out <dir> -Notes "..."    the notes latest.json carries (the
#                                                           CHANGELOG section's intro); "Luna <version>" without
#   ./scripts/release-bundle.ps1 -Out <dir> -Unsigned       after an -Unsigned build: no .sig,
#                                                           and latest.json with an empty signature
#
# -Out has to be empty or not there yet: nothing in it is overwritten or deleted.
param(
    [Parameter(Mandatory)][string]$Out,
    [string]$Notes,
    [switch]$Unsigned
)
. "$PSScriptRoot/lib.ps1"
. "$PSScriptRoot/release-lib.ps1"

$version = Get-ReleaseVersion
$built = Find-ReleaseArtifacts $version -Unsigned:$Unsigned

# The core fingerprint build.rs left beside the exe, from the value the exe is
# stamped with. Where the exe can run, it is asked too: the two differing would
# publish a fingerprint that is not the update's (coreproc.rs holds them to each
# other in a test as well).
$fingerprint = (Get-Content -LiteralPath $built.Fingerprint -Raw).Trim()
Assert-CoreFingerprint $fingerprint $built.Fingerprint
if ($IsWindows) {
    $asked = Join-Path ([IO.Path]::GetDirectoryName($built.Exe)) 'core-fingerprint.reported.txt'
    Remove-Item -LiteralPath $asked -ErrorAction SilentlyContinue
    Start-Process -FilePath $built.Exe -ArgumentList '--core-fingerprint', "`"$asked`"" -Wait
    if (-not (Test-Path -LiteralPath $asked)) { throw 'the exe did not report its core fingerprint' }
    $reported = (Get-Content -LiteralPath $asked -Raw).Trim()
    Remove-Item -LiteralPath $asked
    if ($reported -cne $fingerprint) {
        throw "the exe reports core fingerprint $reported, but build.rs left $fingerprint beside it"
    }
}

$Out = [IO.Path]::GetFullPath($Out, (Get-Location).Path)
if (Test-Path -LiteralPath $Out) {
    if (Get-ChildItem -LiteralPath $Out -Force) { throw "$Out is not empty" }
} else {
    New-Item -ItemType Directory -Force $Out | Out-Null
}

$setupName = Split-Path -Leaf $built.Setup
Copy-Item -LiteralPath $built.Setup -Destination $Out
$signature = ''
if ($built.Signature) {
    Copy-Item -LiteralPath $built.Signature -Destination $Out
    $signature = (Get-Content -LiteralPath $built.Signature -Raw).Trim()
} else {
    Write-Warning 'unsigned build: latest.json carries an empty signature, which no installed Luna would accept'
}

$manifest = New-UpdaterManifest -Version $version -CoreFingerprint $fingerprint -Signature $signature `
    -SetupName $setupName -Endpoint (Get-UpdaterEndpoint) -Notes $Notes
$manifest | Set-Content (Join-Path $Out 'latest.json') -Encoding utf8

Write-Host "release bundle for v$version (core $fingerprint) -> $Out"
Get-ChildItem -LiteralPath $Out | ForEach-Object { Write-Host "  $($_.Name)" }
