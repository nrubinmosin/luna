#Requires -Version 7.4
# Prepares a release; GitHub Actions builds, signs and drafts it from the tag
# (.github/workflows/release.yml, docs/releasing.md). The version is written in
# package.json, src-tauri/tauri.conf.json, src-tauri/Cargo.toml and Cargo.lock.
#
#   ./scripts/release.ps1 -Bump 1.2.3       write 1.2.3 into all four, and nothing else
#   ./scripts/release.ps1                   check the pushed HEAD and tag it v<version>
#   ./scripts/release.ps1 -DryRun           check everything, tag nothing
#
# The release notes are the version's section of CHANGELOG.md (release-lib.ps1
# New-ReleaseNotes), so the tag is refused while that section is missing or
# empty. The tag is made here and pushed by hand, with the command the script
# prints: the push is what starts the workflow.
#
# -Local cuts the release on this machine instead, the way it was done before
# the workflow, for when the workflow cannot (no signing secrets yet): it builds
# the installer here, signs it apart from the build as the workflow does
# (release-sign.ps1), writes latest.json and uploads a published release with
# gh, its notes from CHANGELOG.md too, without the verify section a release
# with SBOMs and attestations gets.
#
#   ./scripts/release.ps1 -Local                release the current version
#   ./scripts/release.ps1 -Local -DryRun        build, sign and compose, upload nothing
#   ./scripts/release.ps1 -Local -SkipBuild     sign and upload what the last build of this
#                                               commit left behind, e.g. after a failed upload
#
# -Local signs with the updater's private key: -UpdaterKey, or LUNA_UPDATER_KEY,
# names it. The build never sees it, nor this checkout: it builds an export of
# the commit, and nothing it made is run here. Only release-sign.ps1's
# container sees the key. A -Local build starts from a cold target folder.
# -TrustedPubkey (or LUNA_UPDATER_TRUSTED_PUBKEY) is for a key rotation's
# bridge release: the old public key the signature is checked against
# (release-sign.ps1, docs/releasing.md).
[CmdletBinding(DefaultParameterSetName = 'Tag')]
param(
    [Parameter(ParameterSetName = 'Bump', Mandatory)][string]$Bump,
    [Parameter(ParameterSetName = 'Local', Mandatory)][switch]$Local,
    [Parameter(ParameterSetName = 'Tag')][Parameter(ParameterSetName = 'Local')][switch]$DryRun,
    [Parameter(ParameterSetName = 'Local')][string]$UpdaterKey = $env:LUNA_UPDATER_KEY,
    [Parameter(ParameterSetName = 'Local')][string]$TrustedPubkey = $env:LUNA_UPDATER_TRUSTED_PUBKEY,
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
        Write-Host 'Next: write its CHANGELOG.md section (the release notes), commit on a branch and'
        Write-Host 'merge it into master through a pull request (master takes no direct pushes):'
        Write-Host "  git switch -c release-$version; git commit -am `"Luna $version`"; git push -u origin release-$version"
        Write-Host 'Once it is merged, release from master as merged:'
        Write-Host '  git switch master; git pull --ff-only; ./scripts/release.ps1'
        return
    }

    $version = Get-ReleaseVersion
    $tag = "v$version"
    Assert-Pushed
    # The notes as the release will carry them, rendered now so a missing or
    # empty section, this version's or one of an unpublished version before
    # it, stops the release before anything is built or tagged.
    $changelog = Get-ChangelogSection $version
    $notes = New-ReleaseNotes -Version $version -Previous (Get-LatestReleaseTag)

    if ($Local) {
        # gh has one exit code for every failure; only its "release not found"
        # means the tag is free. Anything else (auth, network) stops the release here.
        $view = gh release view $tag 2>&1 | ForEach-Object { "$_" }
        if (-not $LASTEXITCODE) { throw "$tag already exists — bump the version." }
        if (-not ($view -match 'release not found')) {
            throw "gh release view $tag failed (exit $LASTEXITCODE): $($view -join ' ')"
        }

        # Before the build, which takes a while, rather than after it.
        if (-not $UpdaterKey -or -not (Test-Path -LiteralPath $UpdaterKey -PathType Leaf)) {
            throw "-Local signs with the updater's private key: pass -UpdaterKey <path> or set LUNA_UPDATER_KEY (now '$UpdaterKey')"
        }

        Write-Host "Releasing $tag from this machine" -ForegroundColor Cyan
        # The build runs every dependency's build code, as root, in a tree it
        # can write; so it builds an export of the commit, never this checkout,
        # whose scripts sign with the key afterwards. Which commit an export
        # holds is written beside it, from here, for -SkipBuild to check.
        $source = Join-Path $RepoRoot 'src-tauri/target/release-source'
        $sourceCommit = "$source.commit"
        $commit = git rev-parse HEAD
        if ($LASTEXITCODE) { throw "git rev-parse failed (exit $LASTEXITCODE)" }
        if ($SkipBuild) {
            $built = if (Test-Path -LiteralPath $sourceCommit) { (Get-Content -LiteralPath $sourceCommit -Raw).Trim() }
            if ($built -cne $commit) { throw "the last -Local build is of '$built', not of HEAD $commit; build it again" }
        } else {
            Remove-Item -LiteralPath $sourceCommit -ErrorAction SilentlyContinue
            Remove-BuiltTree $source
            Export-ReleaseSource $commit $source
            # build-windows.ps1 checks its own native commands: a failed build would
            # otherwise go on to upload whatever the previous build left behind.
            & "$PSScriptRoot/build-windows.ps1" -Installer -Source $source
            Set-Content -LiteralPath $sourceCommit $commit -NoNewline
        }
        # The bare exe goes up no more: the updater installs the setup into the
        # folder Luna was installed in, so a copy run from anywhere else updated
        # into a second Luna with none of its data.
        $bundle = Join-Path $RepoRoot 'src-tauri/target/release-bundle'
        Remove-Item -LiteralPath $bundle -Recurse -Force -ErrorAction SilentlyContinue
        & "$PSScriptRoot/release-bundle.ps1" -Out $bundle -Notes $changelog.Intro -Source $source
        & "$PSScriptRoot/release-sign.ps1" -Dir $bundle -Version $version -UpdaterKey $UpdaterKey -TrustedPubkey $TrustedPubkey
        # Beside the bundle, not in it: everything in the bundle is uploaded.
        $notesFile = Join-Path $RepoRoot 'src-tauri/target/release-notes.md'
        [IO.File]::WriteAllText($notesFile, $notes, [Text.UTF8Encoding]::new($false))

        if ($DryRun) {
            Write-Host "Dry run: nothing uploaded; the notes are in $notesFile." -ForegroundColor Yellow
            return
        }

        $assets = (Get-ChildItem -LiteralPath $bundle -File).FullName
        gh release create $tag @assets --title "Luna $version" --notes-file $notesFile --target master
        # How v0.4.2 first went out: gh timed out on the TLS handshake, created
        # nothing, and the script still said Released.
        if ($LASTEXITCODE) {
            throw "gh release create failed (exit $LASTEXITCODE). If 'gh release view $tag' finds nothing, rerun with -SkipBuild; if the release is there with assets missing, finish it with 'gh release upload $tag'."
        }
        Write-Host "Released $tag" -ForegroundColor Green
        return
    }

    # Free here and on origin. ls-remote prints nothing for a missing tag, so
    # its exit code is read first.
    git rev-parse --quiet --verify "refs/tags/$tag" | Out-Null
    if (-not $LASTEXITCODE) { throw "$tag already exists here — bump the version, or delete the stale tag." }
    $remote = git ls-remote --tags origin "refs/tags/$tag"
    if ($LASTEXITCODE) { throw "git ls-remote origin failed (exit $LASTEXITCODE)" }
    if ($remote) { throw "$tag already exists on origin — bump the version." }

    # Annotated for its tagger and date; the notes are the CHANGELOG's.
    $message = "Luna $version"
    $head = git rev-parse --short HEAD
    if ($LASTEXITCODE) { throw "git rev-parse failed (exit $LASTEXITCODE)" }
    if ($DryRun) {
        Write-Host "Dry run: $tag would tag $head; the release notes open with: $($changelog.Intro)" -ForegroundColor Yellow
        return
    }
    Invoke-Checked 'git tag' { git tag --annotate $tag --message $message }
    Write-Host "Tagged $head as $tag. Push the tag to start the release workflow:" -ForegroundColor Green
    Write-Host "  git push origin $tag"
} finally {
    Pop-Location
}
