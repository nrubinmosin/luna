#Requires -Version 7.4
# Prepares a release; GitHub Actions builds, signs and drafts it from the tag
# (.github/workflows/release.yml, docs/releasing.md). The version is written in
# package.json, src-tauri/tauri.conf.json, src-tauri/Cargo.toml and Cargo.lock.
#
#   ./scripts/release.ps1 -Bump 1.2.3       write 1.2.3 into all four, and nothing else
#   ./scripts/release.ps1                   check the pushed HEAD and tag it v<version>
#   ./scripts/release.ps1 -Notes "..."      the tag's message, which becomes the release notes
#   ./scripts/release.ps1 -DryRun           check everything, tag nothing
#
# The tag is made here and pushed by hand, with the command the script prints:
# the push is what starts the workflow.
#
# -Local cuts the release on this machine instead, the way it was done before
# the workflow, for when the workflow cannot (no signing secrets yet): it builds
# the signed installer here, writes latest.json and uploads a published release
# with gh.
#
#   ./scripts/release.ps1 -Local                release the current version
#   ./scripts/release.ps1 -Local -Notes "..."   with release notes of your own
#   ./scripts/release.ps1 -Local -DryRun        build and compose, upload nothing
#   ./scripts/release.ps1 -Local -SkipBuild     upload what the last build of this commit
#                                               left behind, e.g. after a failed upload
#
# -Local signs with the updater's private key: -UpdaterKey, or LUNA_UPDATER_KEY,
# names it (build-windows.ps1).
[CmdletBinding(DefaultParameterSetName = 'Tag')]
param(
    [Parameter(ParameterSetName = 'Bump', Mandatory)][string]$Bump,
    [Parameter(ParameterSetName = 'Local', Mandatory)][switch]$Local,
    [Parameter(ParameterSetName = 'Tag')][Parameter(ParameterSetName = 'Local')][string]$Notes,
    [Parameter(ParameterSetName = 'Tag')][Parameter(ParameterSetName = 'Local')][switch]$DryRun,
    [Parameter(ParameterSetName = 'Local')][string]$UpdaterKey = $env:LUNA_UPDATER_KEY,
    [Parameter(ParameterSetName = 'Local')][switch]$SkipBuild
)
. "$PSScriptRoot/lib.ps1"
. "$PSScriptRoot/release-lib.ps1"

# A release has to be reproducible from a commit, and the build stamp baked into
# the binary is only meaningful if the tree it was built from is the tree that
# was pushed; the workflow, besides, only releases a commit master has and CI
# has passed. Each answer is read only once its command has succeeded: a git
# that fails prints nothing on stdout, which would read as all clear.
function Assert-Pushed {
    $changes = git status --porcelain
    if ($LASTEXITCODE) { throw "git status failed (exit $LASTEXITCODE)" }
    if ($changes) { throw 'Working tree is dirty — commit before releasing.' }
    $ahead = git rev-list --count origin/master..HEAD
    if ($LASTEXITCODE) { throw "comparing HEAD with origin/master failed (exit $LASTEXITCODE) — is origin/master fetched?" }
    if ([int]$ahead) { throw 'HEAD is ahead of origin/master — merge it through a pull request, then release from master as merged.' }
}

# Every path below, and the repo gh and git work on, is the checkout's.
Push-Location $RepoRoot
try {
    if ($PSCmdlet.ParameterSetName -eq 'Bump') {
        Set-ReleaseVersion $Bump
        $version = Get-ReleaseVersion
        Write-Host "Version files now say $version." -ForegroundColor Green
        Write-Host 'Next: add its CHANGELOG entry if there is a CHANGELOG.md, commit on a branch and'
        Write-Host 'merge it into master through a pull request (master takes no direct pushes):'
        Write-Host "  git switch -c release-$version; git commit -am `"Luna $version`"; git push -u origin release-$version"
        Write-Host 'Once it is merged, release from master as merged:'
        Write-Host '  git switch master; git pull --ff-only; ./scripts/release.ps1'
        return
    }

    $version = Get-ReleaseVersion
    $tag = "v$version"
    Assert-Pushed

    if ($Local) {
        # gh has one exit code for every failure; only its "release not found"
        # means the tag is free. Anything else (auth, network) stops the release here.
        $view = gh release view $tag 2>&1 | ForEach-Object { "$_" }
        if (-not $LASTEXITCODE) { throw "$tag already exists — bump the version." }
        if (-not ($view -match 'release not found')) {
            throw "gh release view $tag failed (exit $LASTEXITCODE): $($view -join ' ')"
        }

        Write-Host "Releasing $tag from this machine" -ForegroundColor Cyan
        # build-windows.ps1 checks its own native commands: a failed build would
        # otherwise go on to upload whatever the previous build left behind.
        if (-not $SkipBuild) {
            & "$PSScriptRoot/build-windows.ps1" -Installer -UpdaterKey $UpdaterKey
        }
        # The bare exe goes up no more: the updater installs the setup into the
        # folder Luna was installed in, so a copy run from anywhere else updated
        # into a second Luna with none of its data.
        $bundle = Join-Path $RepoRoot 'src-tauri/target/release-bundle'
        Remove-Item -LiteralPath $bundle -Recurse -Force -ErrorAction SilentlyContinue
        & "$PSScriptRoot/release-bundle.ps1" -Out $bundle -Notes $Notes

        if ($DryRun) {
            Write-Host 'Dry run: nothing uploaded.' -ForegroundColor Yellow
            return
        }

        $notesBody = if ($Notes) { $Notes } else { "Luna $version" }
        $assets = (Get-ChildItem -LiteralPath $bundle -File).FullName
        gh release create $tag @assets --title "Luna $version" --notes $notesBody --target master
        # How v0.4.2 first went out: gh timed out on the TLS handshake, created
        # nothing, and the script still said Released.
        if ($LASTEXITCODE) {
            throw "gh release create failed (exit $LASTEXITCODE). If 'gh release view $tag' finds nothing, rerun with -SkipBuild; if the release is there with assets missing, finish it with 'gh release upload $tag'."
        }
        Write-Host "Released $tag" -ForegroundColor Green
        return
    }

    $changelog = Test-ChangelogHasVersion $version
    if ($null -eq $changelog) {
        Write-Warning 'no CHANGELOG.md to check for this version'
    } elseif (-not $changelog) {
        throw "CHANGELOG.md has no heading for $version — add its entry first."
    }

    # Free here and on origin. ls-remote prints nothing for a missing tag, so
    # its exit code is read first.
    git rev-parse --quiet --verify "refs/tags/$tag" | Out-Null
    if (-not $LASTEXITCODE) { throw "$tag already exists here — bump the version, or delete the stale tag." }
    $remote = git ls-remote --tags origin "refs/tags/$tag"
    if ($LASTEXITCODE) { throw "git ls-remote origin failed (exit $LASTEXITCODE)" }
    if ($remote) { throw "$tag already exists on origin — bump the version." }

    $message = if ($Notes) { $Notes } else { "Luna $version" }
    $head = git rev-parse --short HEAD
    if ($LASTEXITCODE) { throw "git rev-parse failed (exit $LASTEXITCODE)" }
    if ($DryRun) {
        Write-Host "Dry run: $tag would tag $head with: $message" -ForegroundColor Yellow
        return
    }
    Invoke-Checked 'git tag' { git tag --annotate $tag --message $message }
    Write-Host "Tagged $head as $tag. Push the tag to start the release workflow:" -ForegroundColor Green
    Write-Host "  git push origin $tag"
} finally {
    Pop-Location
}
