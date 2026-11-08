#Requires -Version 7.4
# The release notes of the version in the version files, made from CHANGELOG.md
# (New-ReleaseNotes in release-lib.ps1) and written to a file for
# `gh release create --notes-file`: the version's section; the sections of
# every CHANGELOG version newer than the latest published release, so a
# version tagged but never published ships its changes here; the compare
# link; and, with -Checksums, a Verify section with the commands
# docs/releasing.md documents and the SHA256SUMS themselves.
#
#   ./scripts/release-notes.ps1 -Repo owner/name -Checksums <SHA256SUMS> -Out <file>
#       as the release workflow runs it: the latest published release is asked of
#       GitHub through gh (GH_TOKEN)
#   ./scripts/release-notes.ps1 -Previous v1.2.2 -Out <file>
#       with the previous release named ('' for none), nothing asked of anyone
param(
    [Parameter(Mandatory)][string]$Out,
    [string]$Repo,
    [AllowEmptyString()][string]$Previous,
    [string]$Checksums
)
. "$PSScriptRoot/lib.ps1"
. "$PSScriptRoot/release-lib.ps1"

if ($PSBoundParameters.ContainsKey('Repo') -eq $PSBoundParameters.ContainsKey('Previous')) {
    throw 'name the previous release one way: -Repo owner/name (asked of GitHub) or -Previous <tag>'
}
$version = Get-ReleaseVersion
if ($Repo) { $Previous = Get-LatestReleaseTag $Repo }
$sums = if ($Checksums) { Get-Content -LiteralPath $Checksums -Raw } else { $null }

$notes = New-ReleaseNotes -Version $version -Previous $Previous -Checksums $sums
$Out = [IO.Path]::GetFullPath($Out, (Get-Location).Path)
[IO.File]::WriteAllText($Out, $notes, [Text.UTF8Encoding]::new($false))
Write-Host "release notes for v$version, since $(if ($Previous) { $Previous } else { 'no published release' }) -> $Out"
