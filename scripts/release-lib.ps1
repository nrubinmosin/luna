# What the release scripts share: release.ps1, release-bundle.ps1,
# release-verify.ps1 and sbom.ps1, and through them the release workflow
# (.github/workflows/release.yml). Each dot-sources it after lib.ps1:
#
#   . "$PSScriptRoot/lib.ps1"
#   . "$PSScriptRoot/release-lib.ps1"
#
# The functions that only compute take the repo root or their inputs as
# parameters, so release-lib.test.ps1 (run by the gate) can try them on a tree
# of its own.

# The four places a release's version is written. Each is read and written by
# the line that holds it, so a bump leaves the rest of the file, its line
# endings included, as it was: package.json and tauri.conf.json at their top
# level, Cargo.toml in its [package] table, Cargo.lock in luna's own entry.
$ReleaseVersionFiles = [ordered]@{
    'package.json'              = 'json'
    'src-tauri/tauri.conf.json' = 'json'
    'src-tauri/Cargo.toml'      = 'cargo-toml'
    'src-tauri/Cargo.lock'      = 'cargo-lock'
}

# The version line of one of those files: the match of a pattern whose groups
# are the text before the version, the version and the text after it.
function Find-VersionLine([string]$Text, [string]$Kind) {
    $pattern = switch ($Kind) {
        # The first "version" key is the top-level one in both files: the
        # build section and the plugins come after it, and neither has one.
        'json' { '(?m)^(\s*"version"\s*:\s*")([^"]*)(")' }
        # The first version key after the [package] header, before any other table.
        'cargo-toml' { '(?m)^(\[package\]\r?\n(?:(?!\[).*\n)*?version\s*=\s*")([^"]*)(")' }
        'cargo-lock' { '(?m)^(name = "luna"\r?\nversion = ")([^"]*)(")' }
    }
    [regex]::Match($Text, $pattern)
}

# Each file's version, by its path from the root.
function Get-ReleaseVersions([string]$Root = $RepoRoot) {
    $found = [ordered]@{}
    foreach ($file in $ReleaseVersionFiles.Keys) {
        $text = [IO.File]::ReadAllText((Join-Path $Root $file))
        $line = Find-VersionLine $text $ReleaseVersionFiles[$file]
        if (-not $line.Success) { throw "no version found in $file" }
        $found[$file] = $line.Groups[2].Value
    }
    $found
}

# The one version all four files agree on; throws, naming each, when they do not.
function Get-ReleaseVersion([string]$Root = $RepoRoot) {
    $found = Get-ReleaseVersions $Root
    $versions = @($found.Values | Sort-Object -Unique)
    if ($versions.Count -ne 1) {
        $each = ($found.Keys | ForEach-Object { "$_ $($found[$_])" }) -join ', '
        throw "the version files disagree: $each (./scripts/release.ps1 -Bump <version> sets all four)"
    }
    $versions[0]
}

# Semver as the updater compares it: x.y.z with an optional pre-release part.
function Assert-VersionFormat([string]$Version) {
    if ($Version -notmatch '^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$') {
        throw "'$Version' is not a version of the form 1.2.3 (or 1.2.3-rc.1)"
    }
}

# Writes $Version into all four files, each only on its version line.
function Set-ReleaseVersion([string]$Version, [string]$Root = $RepoRoot) {
    Assert-VersionFormat $Version
    # Every file is read and matched before any is written, so a file that
    # cannot be bumped leaves the other three as they were.
    $edits = foreach ($file in $ReleaseVersionFiles.Keys) {
        $path = Join-Path $Root $file
        $text = [IO.File]::ReadAllText($path)
        $line = Find-VersionLine $text $ReleaseVersionFiles[$file]
        if (-not $line.Success) { throw "no version found in $file" }
        $value = $line.Groups[1].Value + $Version + $line.Groups[3].Value
        @{ Path = $path; Text = $text.Remove($line.Index, $line.Length).Insert($line.Index, $value) }
    }
    $utf8 = [Text.UTF8Encoding]::new($false)
    foreach ($edit in $edits) { [IO.File]::WriteAllText($edit.Path, $edit.Text, $utf8) }
}

# What a run of the release workflow makes of the ref it runs on. A v* tag is
# the release of that version: signed, and drafted on GitHub unless the run is
# a dry run. Any other ref may only be a dry run, and builds unsigned: the
# signing key is kept for v* tags (the `release` environment, docs/releasing.md).
function Get-ReleasePlan([string]$Ref, [string]$Version, [bool]$DryRun) {
    if ($Ref.StartsWith('refs/tags/')) {
        $tag = $Ref.Substring('refs/tags/'.Length)
        if ($tag -cne "v$Version") {
            throw "tag $tag does not name the version the version files hold, ${Version} (tag v$Version)"
        }
        return [pscustomobject]@{ Tag = $tag; Signed = $true; Publish = -not $DryRun }
    }
    if (-not $DryRun) {
        throw "a release runs from its v* tag, not from $Ref; push the tag ./scripts/release.ps1 made, or run with dry-run"
    }
    [pscustomobject]@{ Tag = "v$Version"; Signed = $false; Publish = $false }
}

# The verdict on a commit's CI from its runs of ci.yml (the runs API's
# workflow_runs, push events only): 'green' once one has succeeded, 'pending'
# while one is still going and none has, 'red' once all are done and none
# succeeded, 'missing' while there are none.
function Get-CiVerdict([object[]]$Runs) {
    $Runs = @($Runs | Where-Object { $_ })
    if (-not $Runs) { return 'missing' }
    if ($Runs | Where-Object { $_.status -eq 'completed' -and $_.conclusion -eq 'success' }) { return 'green' }
    if ($Runs | Where-Object { $_.status -ne 'completed' }) { return 'pending' }
    'red'
}

# Two versions in the order the updater and semver put them: negative when
# $A is the older, zero, positive. A release outranks its own pre-releases.
function Compare-ReleaseVersion([string]$Left, [string]$Right) {
    $l = @($Left -split '-', 2); $r = @($Right -split '-', 2)
    $order = ([version]$l[0]).CompareTo([version]$r[0])
    if ($order -ne 0) { return $order }
    if ($l.Count -ne $r.Count) { return $(if ($l.Count -eq 1) { 1 } else { -1 }) }
    if ($l.Count -eq 1) { return 0 }
    [string]::CompareOrdinal($l[1], $r[1])
}

# A version heading of CHANGELOG.md, Keep a Changelog style: "## [1.2.3] - 2026-11-08".
$ChangelogHeading = '^## \[(?<version>[^\]]+)\](?: - (?<date>\S+))?\s*$'

# CHANGELOG.md's lines; the release notes come from it, so a tree without one
# cannot be released.
function Read-Changelog([string]$Root) {
    $path = Join-Path $Root 'CHANGELOG.md'
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { throw "no CHANGELOG.md in $Root; the release notes are made from it" }
    [IO.File]::ReadAllText($path) -split "\r?\n"
}

# The versions CHANGELOG.md has sections for, in the file's order (newest
# first); [Unreleased] is not one.
function Get-ChangelogVersions([string]$Root = $RepoRoot) {
    @(Read-Changelog $Root | ForEach-Object {
        if ($_ -match $ChangelogHeading -and $Matches.version -ne 'Unreleased') { $Matches.version }
    })
}

# One version's section of CHANGELOG.md: Version, Date, Heading (the line's
# text after "## "), Intro (the text before its first ### subsection, which
# latest.json carries for the update chip) and Body (intro and subsections as
# written; the link definitions at the end of the file left out). Throws when
# the heading is missing or the section holds no text or bullets, and for
# [Unreleased], whose content is not released as it is.
function Get-ChangelogSection([string]$Version, [string]$Root = $RepoRoot) {
    Assert-VersionFormat $Version
    $lines = Read-Changelog $Root
    $start = -1
    for ($i = 0; $i -lt $lines.Count; $i++) {
        if ($lines[$i] -match $ChangelogHeading -and $Matches.version -ceq $Version) { $start = $i; $date = $Matches.date; break }
    }
    if ($start -lt 0) { throw "CHANGELOG.md has no section for $Version (## [$Version] - <date>); write its entry first" }
    $body = foreach ($line in $lines[($start + 1)..($lines.Count - 1)]) {
        if ($line -match '^## ') { break }
        if ($line -match '^\[[^\]]+\]:\s*\S+\s*$') { continue }
        $line
    }
    $text = (@($body) -join "`n").Trim()
    if (-not @($text -split "`n" | Where-Object { $_.Trim() -and $_ -notmatch '^###' })) {
        throw "the $Version section of CHANGELOG.md is empty; write what changed before releasing it"
    }
    [pscustomobject]@{
        Version = $Version
        Date    = $date
        Heading = $lines[$start].Substring(3).Trim()
        Intro   = (($text -split '(?m)^###', 2)[0]).Trim()
        Body    = $text
    }
}

# The release notes of $Version, from CHANGELOG.md: its section as written;
# then, under a heading each, every CHANGELOG version newer than $Previous,
# the tag of the latest published release (v0.10.0 was tagged and never
# published, so 0.10.1's notes carry its changes too; '' when nothing has
# been published yet, and then $Version's section alone); the compare view
# and the CHANGELOG, linked; and with -Checksums (the SHA256SUMS text) a
# Verify section with the commands docs/releasing.md documents and the
# checksums themselves. Reads nothing but the tree at $Root. Returns the
# Markdown text.
function New-ReleaseNotes {
    param(
        [Parameter(Mandatory)][string]$Version,
        [AllowEmptyString()][string]$Previous = '',
        [string]$Checksums,
        [string]$Root = $RepoRoot
    )
    $repo = Get-RepoUrl $Root
    $section = Get-ChangelogSection $Version $Root
    $parts = @($section.Body)
    if ($Previous) {
        $versions = Get-ChangelogVersions $Root
        $previousVersion = $Previous -replace '^v', ''
        for ($i = [array]::IndexOf($versions, $Version) + 1; $i -lt $versions.Count; $i++) {
            if ((Compare-ReleaseVersion $versions[$i] $previousVersion) -le 0) { break }
            $older = Get-ChangelogSection $versions[$i] $Root
            $parts += "## Also from $($older.Version)`n`n$($older.Version) had no published release of its own; its changes first ship here.`n`n$($older.Body)"
        }
    }
    # GitHub's anchor for the heading: lowercased, punctuation but hyphens
    # dropped, blanks to hyphens.
    $anchor = ($section.Heading.ToLowerInvariant() -replace '[^\w\s-]', '') -replace '\s', '-'
    $changelog = "[CHANGELOG]($repo/blob/v$Version/CHANGELOG.md#$anchor)"
    $parts += if ($Previous) {
        "Full changes: [$Previous...v$Version]($repo/compare/$Previous...v$Version) and the $changelog."
    } else {
        "Full changes: the $changelog."
    }
    if ($Checksums) {
        $setup = $null
        foreach ($line in $Checksums -split "\r?\n") {
            if ($line -match '^\S+\s+\*?(\S+-setup\.exe)$') { $setup = $Matches[1]; break }
        }
        if (-not $setup) { throw 'SHA256SUMS names no *-setup.exe; the verify commands in the notes need the installer' }
        $slug = $repo -replace '^https://github\.com/', ''
        $fence = '```'
        $parts += @"
## Verify

${fence}sh
# Built by this repository's release workflow, from the tagged commit.
gh attestation verify $setup --repo $slug \
  --signer-workflow $slug/.github/workflows/release.yml

# The SBOMs attested against the installer.
gh attestation verify $setup --repo $slug \
  --predicate-type https://cyclonedx.org/bom

# Every asset against the checksums.
sha256sum -c SHA256SUMS
$fence

SHA256SUMS:

${fence}text
$($Checksums.Trim())
$fence
"@
    }
    (($parts -join "`n`n") -replace "`r`n", "`n") + "`n"
}

# The folder the Windows release build leaves its artifacts in
# (build-windows.ps1).
$ReleaseBuildDir = Join-Path $RepoRoot 'src-tauri/target/x86_64-pc-windows-msvc/release'

# The release build's artifacts for $Version: the exe, the installer, its
# updater signature ($null for an -Unsigned build) and the core fingerprint
# build.rs left beside the exe. Throws when one is missing.
function Find-ReleaseArtifacts([string]$Version, [switch]$Unsigned, [string]$Dir = $ReleaseBuildDir) {
    # By version, not by whatever comes first: the bundle folder keeps every
    # installer ever built there, and picking the first one alphabetically is
    # how v0.2.0 went out carrying the 0.1.0 setup and its signature, an update
    # that offered a build its own version.
    $setup = Get-ChildItem (Join-Path $Dir 'bundle/nsis') -Filter "*_${Version}_*-setup.exe" -ErrorAction SilentlyContinue |
        Select-Object -First 1
    if (-not $setup) { throw "no installer for $Version in $Dir/bundle/nsis" }
    $found = [pscustomobject]@{
        Exe         = Join-Path $Dir 'luna.exe'
        Setup       = $setup.FullName
        Signature   = if ($Unsigned) { $null } else { "$($setup.FullName).sig" }
        Fingerprint = Join-Path $Dir 'core-fingerprint.txt'
    }
    foreach ($f in @($found.Exe, $found.Setup, $found.Signature, $found.Fingerprint) | Where-Object { $_ }) {
        if (-not (Test-Path -LiteralPath $f -PathType Leaf)) { throw "missing build artifact: $f" }
    }
    $found
}

# A core fingerprint as build.rs writes it: 16 lowercase hex digits.
function Assert-CoreFingerprint([string]$Fingerprint, [string]$From) {
    if ($Fingerprint -cnotmatch '^[0-9a-f]{16}$') { throw "$From holds '$Fingerprint', not a core fingerprint" }
}

# The latest.json the updater plugin fetches: version, the core fingerprint
# (a window whose core has the same one installs the update by restarting only
# itself, updates.store.ts), the notes the update chip shows (the CHANGELOG
# section's intro, Get-ChangelogSection), the date, and the
# installer with its signature, which the plugin checks against the public key
# in tauri.conf.json. The installer is downloaded from the release the
# manifest is published with, in the repo the updater endpoint names: the
# endpoint, which every installed Luna polls, is the one place the repo is
# written. Returns the JSON text.
function New-UpdaterManifest {
    param(
        [Parameter(Mandatory)][string]$Version,
        [Parameter(Mandatory)][string]$CoreFingerprint,
        [Parameter(Mandatory)][AllowEmptyString()][string]$Signature,
        [Parameter(Mandatory)][string]$SetupName,
        [Parameter(Mandatory)][string]$Endpoint,
        [string]$Notes,
        [datetime]$PubDate = (Get-Date)
    )
    if ($Endpoint -notmatch '^(https://github\.com/[^/]+/[^/]+/releases)/latest/download/latest\.json$') {
        throw "the updater endpoint $Endpoint is not a GitHub latest-release latest.json"
    }
    $releases = $Matches[1]
    $manifest = [ordered]@{
        version          = $Version
        core_fingerprint = $CoreFingerprint
        notes            = if ($Notes) { $Notes } else { "Luna $Version" }
        pub_date         = $PubDate.ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ', [cultureinfo]::InvariantCulture)
        platforms        = [ordered]@{
            'windows-x86_64' = [ordered]@{
                signature = $Signature
                url       = "$releases/download/v$Version/$SetupName"
            }
        }
    }
    $manifest | ConvertTo-Json -Depth 5
}

# An SBOM as the release workflow's attestation step takes it. actions/attest
# (src/sbom.ts, checkIsCycloneDX) calls a file CycloneDX only with bomFormat,
# serialNumber and specVersion all set, and refuses anything else as
# "Unsupported SBOM format"; the check is repeated here so sbom.ps1 stops a
# release at the SBOM, not after the installer is built and signed. Throws
# naming the file and what it lacks.
function Assert-AttestableSbom([string]$Path) {
    $bom = Get-Content -LiteralPath $Path -Raw | ConvertFrom-Json -AsHashtable
    if ($bom -isnot [hashtable]) { throw "$Path is not a JSON object" }
    if ($bom.bomFormat -cne 'CycloneDX') { throw "$Path has bomFormat '$($bom.bomFormat)', not CycloneDX" }
    $missing = @('serialNumber', 'specVersion' | Where-Object { [string]::IsNullOrEmpty($bom[$_]) })
    if ($missing) { throw "$Path has no $($missing -join ', '), which actions/attest requires of a CycloneDX SBOM" }
}

# The updater endpoint in tauri.conf.json, which latest.json is published for.
function Get-UpdaterEndpoint([string]$Root = $RepoRoot) {
    $conf = Get-Content (Join-Path $Root 'src-tauri/tauri.conf.json') -Raw | ConvertFrom-Json
    $endpoints = @($conf.plugins.updater.endpoints)
    if ($endpoints.Count -ne 1) { throw "tauri.conf.json should name one updater endpoint, not $($endpoints.Count)" }
    $endpoints[0]
}

# The repository on GitHub, https://github.com/<owner>/<name>, from the
# updater endpoint: the one place the repo is written (New-UpdaterManifest).
function Get-RepoUrl([string]$Root = $RepoRoot) {
    $endpoint = Get-UpdaterEndpoint $Root
    if ($endpoint -notmatch '^(https://github\.com/[^/]+/[^/]+)/releases/latest/download/latest\.json$') {
        throw "the updater endpoint $endpoint is not a GitHub latest-release latest.json"
    }
    $Matches[1]
}

# The tag of the latest published release on GitHub, '' when there is none
# yet: a draft or a pre-release is not one, as installed copies polling
# releases/latest see it. Through gh (GH_TOKEN, or gh's own login); $Repo is
# owner/name, by default the checkout's.
function Get-LatestReleaseTag([string]$Repo = '{owner}/{repo}') {
    $out = gh api "repos/$Repo/releases/latest" --jq .tag_name 2>&1 | ForEach-Object { "$_" }
    if (-not $LASTEXITCODE) { return ($out -join '').Trim() }
    if ($out -match 'HTTP 404') { return '' }
    throw "gh api repos/$Repo/releases/latest failed (exit $LASTEXITCODE): $($out -join ' ')"
}
