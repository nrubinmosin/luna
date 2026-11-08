# What the release scripts share: release.ps1, release-bundle.ps1,
# release-sign.ps1, release-verify.ps1 and sbom.ps1, and through them the
# release workflow (.github/workflows/release.yml). Each dot-sources it after
# lib.ps1:
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

# Two versions in the order semver (2.0.0, section 11) and the updater put
# them: -1 when $Left is the older, 0, 1. A release outranks its own
# pre-releases; pre-release identifiers compare one by one, numeric ones as
# numbers and below alphanumeric ones, and the shorter set is the lower when
# equal so far: rc.2 < rc.10, alpha < alpha.1 < beta.
function Compare-ReleaseVersion([string]$Left, [string]$Right) {
    $l = @($Left -split '-', 2); $r = @($Right -split '-', 2)
    $order = ([version]$l[0]).CompareTo([version]$r[0])
    if ($order -ne 0) { return [math]::Sign($order) }
    if ($l.Count -ne $r.Count) { return $(if ($l.Count -eq 1) { 1 } else { -1 }) }
    if ($l.Count -eq 1) { return 0 }
    $li = @($l[1] -split '\.'); $ri = @($r[1] -split '\.')
    for ($i = 0; $i -lt [math]::Min($li.Count, $ri.Count); $i++) {
        $ln = $li[$i] -match '^\d+$'; $rn = $ri[$i] -match '^\d+$'
        $order = if ($ln -and $rn) { ([bigint]$li[$i]).CompareTo([bigint]$ri[$i]) }
        elseif ($ln) { -1 } elseif ($rn) { 1 }
        else { [string]::CompareOrdinal($li[$i], $ri[$i]) }
        if ($order -ne 0) { return [math]::Sign($order) }
    }
    [math]::Sign($li.Count - $ri.Count)
}

# A version heading of CHANGELOG.md, Keep a Changelog style: "## [1.2.3] - 2026-11-08".
$ChangelogHeading = '^## \[(?<version>[^\]]+)\](?: - (?<date>\S+))?\s*$'

# A line opening a code fence (CommonMark 4.5): three or more backticks, with
# an info string that holds no backtick (so ```code``` is an inline code span,
# not a fence), or three or more tildes with anything after. The fence closes
# on the same character, at least as many, alone on its line.
$MarkdownFenceOpen = '^ {0,3}(?:(?<fence>`{3,})[^`]*|(?<fence>~{3,}).*)$'
function Get-MarkdownFenceClose([string]$Fence) { "^ {0,3}[$($Fence[0])]{$($Fence.Length),}\s*$" }

# A reference-link definition (CommonMark 4.7): the label, a colon, the
# destination (bare or in <>), an optional title in quotes or parentheses.
$MarkdownLinkDefinition = '^ {0,3}\[(?<name>[^\]]+)\]:\s*(?:<[^>]*>|\S+)(?:\s+(?:"[^"]*"|''[^'']*''|\([^)]*\)))?\s*$'

# CHANGELOG.md read once: Sections, in the file's order, each with Version,
# Date, Heading (the line's text after "## "), Lines (its body as written, up
# to the next ## heading; the reference-link definitions taken out) and
# IntroEnd (how many of those lines come before its first ### subsection); and
# Links, every "[name]: url" definition from anywhere in the file, the line
# as written, by label lowercased, the first of a label kept, as CommonMark
# matches them. A heading, a definition or a ### inside a code fence is text,
# not structure ($MarkdownFenceOpen), so a `## heading` shown in a code block
# neither ends a section nor opens one. The release notes come from this
# file, so a tree without one cannot be released.
function Read-Changelog([string]$Root) {
    $path = Join-Path $Root 'CHANGELOG.md'
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { throw "no CHANGELOG.md in $Root; the release notes are made from it" }
    $sections = [System.Collections.Generic.List[object]]::new()
    $links = @{}
    $current = $null
    $fence = $null
    foreach ($line in [IO.File]::ReadAllText($path) -split "\r?\n") {
        if ($fence) {
            if ($line -match (Get-MarkdownFenceClose $fence)) { $fence = $null }
            if ($current) { $current.Lines.Add($line) }
            continue
        }
        if ($line -match $MarkdownFenceOpen) {
            $fence = $Matches.fence
            if ($current) { $current.Lines.Add($line) }
            continue
        }
        if ($line -match $ChangelogHeading) {
            $current = [pscustomobject]@{
                Version  = $Matches.version
                Date     = $Matches.date
                Heading  = $line.Substring(3).Trim()
                Lines    = [System.Collections.Generic.List[string]]::new()
                IntroEnd = -1
            }
            $sections.Add($current)
            continue
        }
        if ($line -match '^## ') { $current = $null; continue }
        if ($line -match $MarkdownLinkDefinition) {
            $label = $Matches.name.ToLowerInvariant()
            if (-not $links.ContainsKey($label)) { $links[$label] = $line.Trim() }
            continue
        }
        if ($current) {
            if ($current.IntroEnd -lt 0 -and $line -match '^### ') { $current.IntroEnd = $current.Lines.Count }
            $current.Lines.Add($line)
        }
    }
    [pscustomobject]@{ Sections = $sections; Links = $links }
}

# The versions CHANGELOG.md has sections for, in the file's order (newest
# first); [Unreleased] is not one.
function Get-ChangelogVersions([string]$Root = $RepoRoot) {
    @((Read-Changelog $Root).Sections | Where-Object { $_.Version -ne 'Unreleased' } | ForEach-Object { $_.Version })
}

# One version's section of CHANGELOG.md: Version, Date, Heading, Intro (the
# text before its first ### subsection, which latest.json carries for the
# update chip) and Body (intro and subsections as written). Throws when the
# heading is missing or the section holds no text or bullets, and for
# [Unreleased], whose content is not released as it is.
function Get-ChangelogSection([string]$Version, [string]$Root = $RepoRoot) {
    Assert-VersionFormat $Version
    $section = (Read-Changelog $Root).Sections | Where-Object { $_.Version -ceq $Version } | Select-Object -First 1
    if (-not $section) { throw "CHANGELOG.md has no section for $Version (## [$Version] - <date>); write its entry first" }
    $text = ($section.Lines -join "`n").Trim()
    if (-not @($text -split "`n" | Where-Object { $_.Trim() -and $_ -notmatch '^###' })) {
        throw "the $Version section of CHANGELOG.md is empty; write what changed before releasing it"
    }
    $introLines = if ($section.IntroEnd -lt 0) { $section.Lines } elseif ($section.IntroEnd -eq 0) { @() } else { $section.Lines.GetRange(0, $section.IntroEnd) }
    [pscustomobject]@{
        Version = $Version
        Date    = $section.Date
        Heading = $section.Heading
        Intro   = (@($introLines) -join "`n").Trim()
        Body    = $text
    }
}

# The reference-link definitions the Markdown $Text uses, as written, in the
# order first used, from $Links as Read-Changelog collected them: a "[name]"
# not followed by "(" refers to one, outside code fences. A section's
# "[details]" would otherwise lose its target once lifted out of the file.
function Get-UsedLinkDefinitions([string]$Text, [hashtable]$Links) {
    $used = [ordered]@{}
    $fence = $null
    foreach ($line in $Text -split "`n") {
        if ($fence) {
            if ($line -match (Get-MarkdownFenceClose $fence)) { $fence = $null }
            continue
        }
        if ($line -match $MarkdownFenceOpen) { $fence = $Matches.fence; continue }
        foreach ($match in [regex]::Matches($line, '\[([^\]]+)\](?!\()')) {
            $name = $match.Groups[1].Value.ToLowerInvariant()
            if ($Links.ContainsKey($name) -and -not $used.Contains($name)) { $used[$name] = $Links[$name] }
        }
    }
    @($used.Values)
}

# The release notes of $Version, from CHANGELOG.md: its section as written;
# then, under a heading each, every CHANGELOG version newer than $Previous,
# the tag of the latest published release (v0.10.0 was tagged and never
# published, so 0.10.1's notes carry its changes too; '' when nothing has
# been published yet, and then $Version's section alone); the compare view
# and the CHANGELOG, linked; with -Checksums (the SHA256SUMS text) a Verify
# section with the commands docs/releasing.md documents and the checksums
# themselves; and the link definitions the sections use. Refuses a $Previous
# that is not older than $Version: releases go forward of the latest
# published one, which installed copies take, so a backport is not a release
# this pipeline makes, and the notes would span backwards. Reads nothing but
# the tree at $Root. Returns the Markdown text.
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
        $previousVersion = $Previous -replace '^v', ''
        if ((Compare-ReleaseVersion $previousVersion $Version) -ge 0) {
            throw "$Previous, the latest published release, is not older than ${Version}: a release goes forward of the one installed copies take (releases/latest). To render the notes against an older baseline anyway, name it with -Previous."
        }
        $versions = Get-ChangelogVersions $Root
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
    # The definitions the lifted sections refer to, once, where Markdown
    # keeps them: at the end.
    $definitions = Get-UsedLinkDefinitions ($parts -join "`n`n") (Read-Changelog $Root).Links
    if ($definitions) { $parts += ($definitions -join "`n") }
    (($parts -join "`n`n") -replace "`r`n", "`n") + "`n"
}

# The folder the Windows release build leaves its artifacts in
# (build-windows.ps1).
$ReleaseBuildDir = Join-Path $RepoRoot 'src-tauri/target/x86_64-pc-windows-msvc/release'

# The tree of $Commit, as git has it, written to the new folder $To: the
# source `release.ps1 -Local` builds, so the build's container, which runs
# every dependency's build code with the tree mounted writable, has no way
# to this checkout, whose scripts then sign. No .git goes with it (build.rs
# takes the stamp from LUNA_BUILD), nor .claude/, whose skill links the
# build needs no more than a Windows extraction could make them.
function Export-ReleaseSource([string]$Commit, [string]$To, [string]$Root = $RepoRoot) {
    if (Test-Path -LiteralPath $To) { throw "$To is there already; the release source goes into a new folder" }
    New-Item -ItemType Directory -Force $To | Out-Null
    $tar = "$To.tar"
    try {
        Invoke-Checked 'git archive' { git -C $Root archive --format=tar -o $tar $Commit -- . ':(exclude).claude' }
        [System.Formats.Tar.TarFile]::ExtractToDirectory($tar, $To, $false)
    } finally {
        Remove-Item -LiteralPath $tar -ErrorAction SilentlyContinue
    }
}

# Deletes $Path, a tree a build in a container wrote, from inside a container
# that sees only its parent: `rm -rf` there takes a link the build left as the
# link, where a delete from this side could follow a junction out of the tree.
function Remove-BuiltTree([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path)) { return }
    $Path = [IO.Path]::GetFullPath($Path)
    $parent = Split-Path -Parent $Path
    $name = Split-Path -Leaf $Path
    Invoke-Checked "removing $Path" {
        docker run --rm --network none -v "${parent}:/parent" (Get-NodeImage) rm -rf -- "/parent/$name"
    }
    if (Test-Path -LiteralPath $Path) { throw "$Path is still there" }
}

# Throws when $Path, or any folder between $Root and it, is a link or
# junction: what a build wrote is read only as the files it is, never
# through a link it could have aimed at the updater key or anything else of
# the user's. $Path has to be under $Root.
function Assert-NoLinkBelow([string]$Root, [string]$Path) {
    $Root = [IO.Path]::GetFullPath($Root).TrimEnd('\', '/')
    $at = [IO.Path]::GetFullPath($Path).TrimEnd('\', '/')
    $sep = [IO.Path]::DirectorySeparatorChar
    if (-not ($at -eq $Root -or $at.StartsWith("$Root$sep", [StringComparison]::OrdinalIgnoreCase) -or $at.StartsWith("$Root/", [StringComparison]::OrdinalIgnoreCase))) {
        throw "$Path is not under $Root"
    }
    while ($at.Length -gt $Root.Length) {
        if (Test-Link $at) { throw "$Path is reached through a link ($at)" }
        $at = [IO.Path]::GetDirectoryName($at)
    }
}

# The release build's artifacts for $Version: the installer and the core
# fingerprint build.rs left beside the exe, found in $Dir, which lies under
# $Root, the tree the build wrote. Throws when one is missing or is reached
# through a link below $Root. Nothing the build made is run: the exe is not
# needed, and asking it for its fingerprint would run whatever a dependency's
# build code put into it, on this machine, as its user. The build signs
# nothing either: the installer's updater signature is made apart from it
# (release-sign.ps1).
function Find-ReleaseArtifacts([string]$Version, [string]$Dir = $ReleaseBuildDir, [string]$Root = $RepoRoot) {
    $nsis = Join-Path $Dir 'bundle/nsis'
    Assert-NoLinkBelow $Root $nsis
    # By version, not by whatever comes first: the bundle folder keeps every
    # installer ever built there, and picking the first one alphabetically is
    # how v0.2.0 went out carrying the 0.1.0 setup and its signature, an update
    # that offered a build its own version.
    $setup = Get-ChildItem -LiteralPath $nsis -Filter "*_${Version}_*-setup.exe" -ErrorAction SilentlyContinue |
        Select-Object -First 1
    if (-not $setup) { throw "no installer for $Version in $nsis" }
    $found = [pscustomobject]@{
        Setup       = $setup.FullName
        Fingerprint = Join-Path $Dir 'core-fingerprint.txt'
    }
    foreach ($f in @($found.Setup, $found.Fingerprint)) {
        Assert-NoLinkBelow $Root $f
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

# latest.json as the build left it, with no signature, made again with the
# installer's updater signature ($Signature, the .sig's text) once
# release-sign.ps1 has made and checked it. The build that wrote $Manifest ran
# every dependency's build code, so only its core fingerprint, notes and date
# are taken from it, each checked, and everything else is New-UpdaterManifest's
# for $Version and $SetupName: a manifest with another version, URL, platform
# or field, or a signature already, is refused. Returns the JSON text.
function Set-UpdaterManifestSignature {
    param(
        [Parameter(Mandatory)][string]$Manifest,
        [Parameter(Mandatory)][string]$Signature,
        [Parameter(Mandatory)][string]$Version,
        [Parameter(Mandatory)][string]$SetupName,
        [Parameter(Mandatory)][string]$Endpoint
    )
    if (-not $Signature.Trim()) { throw 'no signature to put into latest.json' }
    $json = $Manifest | ConvertFrom-Json -AsHashtable
    if ($json -isnot [System.Collections.IDictionary]) { throw 'latest.json is not a JSON object' }
    $fields = @($json.Keys | Sort-Object)
    if (($fields -join ',') -cne 'core_fingerprint,notes,platforms,pub_date,version') {
        throw "latest.json has the fields $($fields -join ', '), not the ones New-UpdaterManifest writes"
    }
    if ($json.version -cne $Version) { throw "latest.json is for version '$($json.version)', not $Version" }
    Assert-CoreFingerprint $json.core_fingerprint 'latest.json'
    $platforms = $json.platforms
    if ($platforms -isnot [System.Collections.IDictionary] -or $platforms.Count -ne 1 -or -not $platforms.Contains('windows-x86_64')) {
        throw 'latest.json should name the one platform windows-x86_64'
    }
    $windows = $platforms['windows-x86_64']
    if ($windows.signature) { throw 'latest.json carries a signature already; the build signs nothing' }
    # ConvertFrom-Json reads an ISO date as a DateTime; anything else stops here.
    $published = $json.pub_date
    if ($published -isnot [datetime]) { throw "latest.json's pub_date '$published' is not a date" }
    $signed = New-UpdaterManifest -Version $Version -CoreFingerprint $json.core_fingerprint -Signature $Signature.Trim() `
        -SetupName $SetupName -Endpoint $Endpoint -Notes $json.notes -PubDate $published
    $expectedUrl = ($signed | ConvertFrom-Json).platforms.'windows-x86_64'.url
    if ($windows.url -cne $expectedUrl) { throw "latest.json points at $($windows.url), not $expectedUrl" }
    $signed
}

# The updater's public key in tauri.conf.json, which every installed Luna
# checks an update's signature against.
function Get-UpdaterPubkey([string]$Root = $RepoRoot) {
    $conf = Get-Content (Join-Path $Root 'src-tauri/tauri.conf.json') -Raw | ConvertFrom-Json
    $pubkey = $conf.plugins.updater.pubkey
    if (-not $pubkey) { throw 'tauri.conf.json has no plugins.updater.pubkey' }
    $pubkey
}

# The one version of the npm package $Name that pnpm-lock.yaml pins, from its
# packages section: Name, Version, Integrity (the SRI sha512 pnpm checks the
# tarball against) and Url, the tarball on the npm registry, where pnpm fetched
# it from (the lockfile names no other tarball, and the repo no other
# registry). Throws when the lockfile holds none, more than one, or one that
# comes from elsewhere.
function Get-LockedNpmPackage([string]$Name, [string]$Root = $RepoRoot) {
    $lock = [IO.File]::ReadAllText((Join-Path $Root 'pnpm-lock.yaml'))
    $entry = "(?m)^  '?$([regex]::Escape($Name))@(?<version>[^':\s(]+)'?:\r?\n    resolution: \{integrity: (?<integrity>sha512-[A-Za-z0-9+/]+=*)\}\r?$"
    $found = [regex]::Matches($lock, $entry)
    if ($found.Count -ne 1) {
        throw "pnpm-lock.yaml should pin one $Name from the npm registry, not $($found.Count)"
    }
    $version = $found[0].Groups['version'].Value
    [pscustomobject]@{
        Name      = $Name
        Version   = $version
        Integrity = $found[0].Groups['integrity'].Value
        Url       = "https://registry.npmjs.org/$Name/-/$(($Name -split '/')[-1])-$version.tgz"
    }
}

# Throws unless the file at $Path has the SRI sha512 $Integrity
# ("sha512-<base64>"), as pnpm-lock.yaml writes it.
function Assert-Sha512Integrity([string]$Path, [string]$Integrity) {
    if ($Integrity -notmatch '^sha512-(?<digest>[A-Za-z0-9+/]+=*)$') { throw "'$Integrity' is not a sha512 integrity" }
    $expected = [Convert]::FromBase64String($Matches.digest)
    $stream = [IO.File]::OpenRead($Path)
    try { $actual = [Security.Cryptography.SHA512]::HashData($stream) } finally { $stream.Dispose() }
    if (-not [Security.Cryptography.CryptographicOperations]::FixedTimeEquals($actual, $expected)) {
        throw "$Path does not match ${Integrity}: it is sha512-$([Convert]::ToBase64String($actual))"
    }
}

# Writes the one regular file $Entry of the gzipped tarball $Tarball to
# $Destination, in-process: no tar of the host's (Windows may find the bsdtar
# or Git's GNU tar first, which take a drive letter for a remote host), and no
# other entry, nor any path the tarball names, gets written. Throws when the
# entry is missing, not a regular file, or there twice.
function Export-TarballEntry([string]$Tarball, [string]$Entry, [string]$Destination) {
    $file = [IO.File]::OpenRead($Tarball)
    try {
        $reader = [System.Formats.Tar.TarReader]::new([IO.Compression.GZipStream]::new($file, [IO.Compression.CompressionMode]::Decompress))
        $written = $false
        while ($null -ne ($item = $reader.GetNextEntry())) {
            if ($item.Name -cne $Entry) { continue }
            if ($written) { throw "$Tarball holds $Entry twice" }
            if ($item.EntryType -notin 'RegularFile', 'V7RegularFile') { throw "$Entry in $Tarball is a $($item.EntryType), not a file" }
            $item.ExtractToFile($Destination, $false)
            $written = $true
        }
        if (-not $written) { throw "$Tarball has no $Entry" }
    } finally {
        $file.Dispose()
    }
}

# The public key a signature is to be checked against, from what the
# releaser gave (-TrustedPubkey, LUNA_UPDATER_TRUSTED_PUBKEY, the `release`
# environment's UPDATER_TRUSTED_PUBKEY): $null, for the key the release
# embeds, when that is empty or `embedded`; else the base64 minisign public
# key given, checked for its shape. -Required refuses an empty value: the
# workflow reads the variable through `vars`, which falls back to a
# repository variable when the environment has none, and repository
# variables any collaborator can write, where only the owner writes the
# environment's. So the environment always defines it, `embedded` but for a
# bridge release, and its value wins over any repository one.
function Resolve-TrustedPubkey([string]$Value, [switch]$Required) {
    $Value = $Value.Trim()
    if (-not $Value) {
        if ($Required) {
            throw 'the release environment defines no UPDATER_TRUSTED_PUBKEY; set it there to `embedded` (or, for a key rotation''s bridge release, to the old public key), so a repository variable of that name cannot stand in for it'
        }
        return $null
    }
    if ($Value -ceq 'embedded') { return $null }
    $lines = try { [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($Value)) -split '\r?\n' } catch { @() }
    $bin = if ($lines.Count -ge 2) { try { [Convert]::FromBase64String($lines[1]) } catch { $null } }
    if (-not $bin -or $bin.Length -ne 42 -or $bin[0] -ne 0x45 -or $bin[1] -notin 0x64, 0x44) {
        throw "the trusted public key '$Value' is not a minisign public key as tauri.conf.json holds one (or `embedded`)"
    }
    $Value
}

# Signs the installer for $Version in the bundle $Dir (release-bundle.ps1's)
# and puts the signature into its latest.json; release-sign.ps1 says why and
# how. In order: the Tauri CLI's Linux build is fetched into a new folder and
# held to the integrity $Root's pnpm-lock.yaml pins before anything of it is
# unpacked, let alone run; `signer sign` runs in the pinned Node image with
# no network, a read-only root and the key; the signature is checked in a
# second container, without the key and with the bundle read-only, against
# $TrustedPubkey, or else the public key $Root's tauri.conf.json embeds; only
# then does latest.json take it. A .sig that fails the check is removed.
#
# $TrustedPubkey is for a key rotation's bridge release, which embeds the new
# public key but is signed with the old private key, the one installed copies
# still trust (docs/releasing.md): the old public key, given by whoever
# releases, never taken from the build. $Fetch ($url, $path) downloads and
# $Run ($what, $dockerArgs) runs `docker run`; both are here for the tests.
# The `docker run` arguments every signer container starts with: removed when
# done, no network, a read-only root, no capability and no privilege to gain.
# On a Unix host it runs as the user who runs this: root without capabilities
# cannot write the .sig into a folder it does not own, and the sign job's
# assets folder is the runner user's (0755). A mount of a Windows folder keeps
# no Unix owner, so there the container stays root.
function Get-SignerIsolation {
    $user = if ($IsWindows) { @() } else { @('--user', "$(id -u):$(id -g)") }
    @('--rm', '--network', 'none', '--read-only', '--cap-drop', 'ALL', '--security-opt', 'no-new-privileges') + $user
}

# A signer container, isolated as Invoke-ReleaseSign runs it, writes a file into
# a folder of this user's that the host then reads: what the sign job does with
# the .sig. The unit tests stand in for docker, so only a real run on a Linux
# host, where Docker holds a container to the folder's owner, shows it.
function Assert-SignerWritesBundle([string]$Root = $RepoRoot) {
    $dir = Join-Path $Root "test-build/signer-probe/$([guid]::NewGuid().ToString('n'))"
    New-Item -ItemType Directory -Force $dir | Out-Null
    try {
        docker run @(Get-SignerIsolation) -v "${dir}:/out" (Get-NodeImage) node -e "require('node:fs').writeFileSync('/out/probe.sig', 'signed')"
        if ($LASTEXITCODE) { throw "the signer's container cannot write into a folder of this user's (exit $LASTEXITCODE)" }
        if ((Get-Content -LiteralPath (Join-Path $dir 'probe.sig') -Raw) -cne 'signed') { throw 'the file the signer''s container wrote did not come back' }
    } finally {
        Remove-Item -LiteralPath $dir -Recurse -Force -ErrorAction SilentlyContinue
    }
}

function Invoke-ReleaseSign {
    param(
        [Parameter(Mandatory)][string]$Dir,
        [Parameter(Mandatory)][string]$Version,
        [string]$UpdaterKey,
        [string]$TrustedPubkey,
        # A key made for this run alone, in the signer's container, signs and
        # is checked against: what a dry run does to go through the signing
        # without the release's key. Nothing it signs is ever published.
        [switch]$Throwaway,
        [string]$Root = $RepoRoot,
        [scriptblock]$Fetch = { param($Url, $Path) Invoke-WebRequest -Uri $Url -OutFile $Path -MaximumRetryCount 3 -RetryIntervalSec 5 },
        [scriptblock]$Run = {
            param($What, $DockerArgs)
            & docker run @DockerArgs
            if ($LASTEXITCODE) { throw "$What failed (exit $LASTEXITCODE)" }
        }
    )
    Assert-VersionFormat $Version
    $Dir = [IO.Path]::GetFullPath($Dir, (Get-Location).Path)
    $setups = @(Get-ChildItem -LiteralPath $Dir -Filter "*_${Version}_*-setup.exe" -File -ErrorAction SilentlyContinue)
    if ($setups.Count -ne 1) { throw "$Dir should hold one installer for $Version, not $($setups.Count)" }
    $setup = $setups[0]
    $sigPath = "$($setup.FullName).sig"
    if (Test-Path -LiteralPath $sigPath) { throw "$($setup.Name) is signed already" }
    $manifestPath = Join-Path $Dir 'latest.json'
    if (-not (Test-Path -LiteralPath $manifestPath -PathType Leaf)) { throw "no latest.json in $Dir" }
    $embedded = Get-UpdaterPubkey $Root
    $pubkey = $embedded
    if (-not $Throwaway -and $TrustedPubkey -and $TrustedPubkey.Trim() -cne $embedded) {
        $pubkey = $TrustedPubkey.Trim()
        Write-Host 'checking the signature against the trusted public key given for a rotation, not the one this release embeds'
    }
    $endpoint = Get-UpdaterEndpoint $Root

    # Passed on by name, its value from this process's environment, so it never
    # shows on a command line. Left out, the CLI takes the key to have no password.
    $keyArgs = @()
    if (-not $Throwaway -and $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD) { $keyArgs += @('-e', 'TAURI_SIGNING_PRIVATE_KEY_PASSWORD') }
    if ($Throwaway) {
        # Made below, once the CLI is in hand.
    } elseif ($UpdaterKey) {
        if (-not (Test-Path -LiteralPath $UpdaterKey -PathType Leaf)) { throw "no updater key at $UpdaterKey" }
        # Mounted rather than passed as an env value, so it never shows in
        # `docker inspect` or the process list on the host.
        $keyPath = (Resolve-Path -LiteralPath $UpdaterKey).Path
        $keyArgs += @('-v', "${keyPath}:/keys/updater.key:ro", '-e', 'TAURI_SIGNING_PRIVATE_KEY_PATH=/keys/updater.key')
    } elseif ($env:TAURI_SIGNING_PRIVATE_KEY) {
        $keyArgs += @('-e', 'TAURI_SIGNING_PRIVATE_KEY')
    } else {
        throw 'signing needs the updater''s private key: pass -UpdaterKey <path>, or set LUNA_UPDATER_KEY or TAURI_SIGNING_PRIVATE_KEY'
    }

    # The CLI's Linux build, the package @tauri-apps/cli installs on a Linux
    # host, as the build's own `pnpm install` fetched and checked it.
    $cli = Get-LockedNpmPackage '@tauri-apps/cli-linux-x64-gnu' $Root
    $work = Join-Path $Root "src-tauri/target/release-signer/$([guid]::NewGuid().ToString('n'))"
    New-Item -ItemType Directory -Force (Join-Path $work 'cli') | Out-Null
    try {
        $tarball = Join-Path $work 'cli.tgz'
        Write-Host "Tauri CLI $($cli.Version) from $($cli.Url)"
        & $Fetch $cli.Url $tarball
        Assert-Sha512Integrity $tarball $cli.Integrity
        Export-TarballEntry $tarball 'package/cli.linux-x64-gnu.node' (Join-Path $work 'cli/cli.node')

        $image = Get-NodeImage
        $isolated = @(Get-SignerIsolation) + @(
            '-v', "$(Join-Path $work 'cli'):/cli:ro",
            '-v', "$(Join-Path $Root 'scripts/updater-signature.mjs'):/sign/updater-signature.mjs:ro"
        )
        if ($Throwaway) {
            $keyDir = New-Item -ItemType Directory -Force (Join-Path $work 'key')
            & $Run 'tauri signer generate' ($isolated + @('-v', "$($keyDir.FullName):/keys", $image,
                    'node', '/sign/updater-signature.mjs', 'generate', '/cli/cli.node', '/keys/updater.key'))
            $pubkey = (Get-Content -LiteralPath (Join-Path $keyDir 'updater.key.pub') -Raw).Trim()
            $keyArgs += @('-v', "$(Join-Path $keyDir 'updater.key'):/keys/updater.key:ro", '-e', 'TAURI_SIGNING_PRIVATE_KEY_PATH=/keys/updater.key')
            Write-Host 'signing with a throwaway key made for this run; its signature is checked against that key''s public half'
        }
        $file = "/out/$($setup.Name)"
        & $Run 'tauri signer sign' ($isolated + $keyArgs + @('-v', "${Dir}:/out", $image,
                'node', '/sign/updater-signature.mjs', 'sign', '/cli/cli.node', $file, $Version))
        try {
            & $Run 'the signature check' ($isolated + @('-v', "${Dir}:/out:ro", $image,
                    'node', '/sign/updater-signature.mjs', 'verify', $file, "$file.sig", $pubkey, $Version))
        } catch {
            Remove-Item -LiteralPath $sigPath -ErrorAction SilentlyContinue
            throw
        }

        $signature = (Get-Content -LiteralPath $sigPath -Raw).Trim()
        $manifest = Set-UpdaterManifestSignature -Manifest (Get-Content -LiteralPath $manifestPath -Raw) -Signature $signature `
            -Version $Version -SetupName $setup.Name -Endpoint $endpoint
        $manifest | Set-Content -LiteralPath $manifestPath -Encoding utf8
        Write-Host "signed $($setup.Name) for v$Version; latest.json carries the signature"
    } finally {
        Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
    }
}

# Throws unless the folder $Dir holds exactly the files $Sums lists (sha256sum
# lines, "<hash>  <name>") and each has its hash: nothing changed, missing or
# added on the way from one job to the next.
function Assert-AssetChecksums([string]$Dir, [string]$Sums) {
    $listed = [ordered]@{}
    foreach ($line in $Sums -split '\r?\n' | Where-Object { $_.Trim() }) {
        if ($line -notmatch '^(?<hash>[0-9a-f]{64}) [ *](?<name>[^/\\]+)$') { throw "not a sha256sum line: '$line'" }
        if ($listed.Contains($Matches.name)) { throw "$($Matches.name) is listed twice" }
        $listed[$Matches.name] = $Matches.hash
    }
    if (-not $listed.Count) { throw 'no checksums to check against' }
    $present = @(Get-ChildItem -LiteralPath $Dir -Force | ForEach-Object { $_.Name })
    $extra = @($present | Where-Object { -not $listed.Contains($_) })
    if ($extra) { throw "$Dir holds what the checksums do not list: $($extra -join ', ')" }
    foreach ($name in $listed.Keys) {
        $path = Join-Path $Dir $name
        if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { throw "$name is missing from $Dir" }
        $hash = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant()
        if ($hash -cne $listed[$name]) { throw "$name has sha256 $hash, not $($listed[$name])" }
    }
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
