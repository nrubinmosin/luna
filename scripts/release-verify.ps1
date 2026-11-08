#Requires -Version 7.4
# The release workflow's first job (.github/workflows/release.yml): decides
# what the run makes of its ref and stops a release that is not ready, before
# anything is built or any secret is in reach.
#
# Every run: the version files agree, and CI (ci.yml) has passed on this very
# commit, as a push; a run still going is waited for, up to -WaitMinutes, so
# the tag can be pushed straight after the commit. A signed run, from a v* tag,
# also needs the tag to name that version, to be annotated, to point at a
# commit on the default branch, and CHANGELOG.md to have the version's section,
# which the release notes are made from.
#
#   ./scripts/release-verify.ps1 -Ref refs/tags/v1.2.3 -Sha <commit> -Repo owner/name
#   ./scripts/release-verify.ps1 -Ref refs/heads/master -Sha <commit> -Repo owner/name -DryRun
#
# Reads only, through gh (GH_TOKEN; the job's token needs contents: read and
# actions: read). Under Actions the answers go to $GITHUB_OUTPUT: version, tag,
# signed, publish, previous (the latest published release's tag, or empty),
# notes.
param(
    [Parameter(Mandatory)][string]$Ref,
    [Parameter(Mandatory)][string]$Sha,
    [Parameter(Mandatory)][string]$Repo,
    [string]$DefaultBranch = 'master',
    [switch]$DryRun,
    [int]$WaitMinutes = 45
)
. "$PSScriptRoot/lib.ps1"
. "$PSScriptRoot/release-lib.ps1"

# A GET on the REST API, parsed; throws on any failure, a 404 included.
function Get-Api([string]$Path) {
    $out = gh api -H 'Accept: application/vnd.github+json' $Path
    if ($LASTEXITCODE) { throw "gh api $Path failed (exit $LASTEXITCODE)" }
    $out | ConvertFrom-Json
}

$version = Get-ReleaseVersion
$plan = Get-ReleasePlan $Ref $version $DryRun.IsPresent
Write-Host "v$version from $Ref at $Sha`: $(if ($plan.Signed) { 'signed' } else { 'unsigned' }), $(if ($plan.Publish) { 'drafted on GitHub' } else { 'dry run' })"

# The latest published release, the baseline the notes span from: looked up
# once here and handed to the publish job, so the two render against the
# same release even if one is published while the run is going.
$previous = Get-LatestReleaseTag $Repo

# The notes latest.json carries, which the update chip shows: the CHANGELOG
# section's intro. The release's own notes are made by the publish job
# (release-notes.ps1) from the same sections, and are rendered here first so
# an empty section, or a version behind the latest published release, fails
# the run before anything is built. A dry run may run on a commit that has
# no section yet; a release may not.
$notes = "Luna $version"
try {
    $section = Get-ChangelogSection $version
    if ($section.Intro) { $notes = $section.Intro }
    New-ReleaseNotes -Version $version -Previous $previous | Out-Null
} catch {
    if ($plan.Signed) { throw }
    Write-Warning "$_; fine for a dry run"
}

if ($plan.Signed) {
    # The tag as GitHub has it, not as the checkout made it: checking out a tag
    # can leave a lightweight copy of an annotated one.
    $tagRef = Get-Api "repos/$Repo/git/ref/tags/$($plan.Tag)"
    if ($tagRef.object.type -ne 'tag') {
        throw "$($plan.Tag) is a lightweight tag; ./scripts/release.ps1 makes an annotated one"
    }
    $tag = Get-Api "repos/$Repo/git/tags/$($tagRef.object.sha)"
    if ($tag.object.type -ne 'commit' -or $tag.object.sha -ne $Sha) {
        throw "$($plan.Tag) points at $($tag.object.type) $($tag.object.sha), not at the commit this run checked out, $Sha"
    }
}

# Contained in the default branch: the compare API calls a commit "behind" (or
# "identical" to) a branch that has it in its history.
$compare = Get-Api "repos/$Repo/compare/$DefaultBranch...$Sha"
if ($compare.status -notin 'identical', 'behind') {
    $what = "$Sha is not on $DefaultBranch ($($compare.status))"
    if ($plan.Signed) { throw "$what; a release is cut from a commit $DefaultBranch has" }
    Write-Warning "$what; fine for a dry run"
}

# A run that has not shown up yet counts as one still going for the first few
# minutes: the tag and the commit may have been pushed together.
$began = Get-Date
while ($true) {
    $runs = (Get-Api "repos/$Repo/actions/workflows/ci.yml/runs?head_sha=$Sha&event=push&per_page=100").workflow_runs
    $verdict = Get-CiVerdict $runs
    $waited = (Get-Date) - $began
    if ($verdict -eq 'green') {
        $run = $runs | Where-Object { $_.conclusion -eq 'success' } | Select-Object -First 1
        Write-Host "CI passed on $Sha`: $($run.html_url)"
        break
    }
    if ($verdict -eq 'red') {
        throw "CI failed on $Sha`: $(($runs | ForEach-Object { $_.html_url }) -join ' ')"
    }
    $giveUp = if ($verdict -eq 'missing') { [math]::Min(5, $WaitMinutes) } else { $WaitMinutes }
    if ($waited.TotalMinutes -ge $giveUp) {
        if ($verdict -eq 'missing') {
            throw "CI has not run on $Sha; push the commit to a branch (CI runs on branch pushes, not on tags)"
        }
        throw "CI on $Sha is still not done after $WaitMinutes minutes"
    }
    Write-Host "CI on $Sha is $verdict; checking again in 30 s"
    Start-Sleep -Seconds 30
}

$outputs = [ordered]@{
    version = $version
    tag     = $plan.Tag
    signed   = "$($plan.Signed)".ToLowerInvariant()
    publish  = "$($plan.Publish)".ToLowerInvariant()
    previous = $previous
}
if ($env:GITHUB_OUTPUT) {
    $lines = foreach ($key in $outputs.Keys) { "$key=$($outputs[$key])" }
    # Notes span lines: the heredoc form, with a delimiter no message holds.
    $eof = "EOF_$([guid]::NewGuid().ToString('n'))"
    $lines += "notes<<$eof", $notes, $eof
    $lines | Add-Content -LiteralPath $env:GITHUB_OUTPUT -Encoding utf8
}
$outputs.GetEnumerator() | ForEach-Object { Write-Host "$($_.Key): $($_.Value)" }
Write-Host "notes: $notes"
