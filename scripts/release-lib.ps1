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

# The places a release's version is written: Cargo.toml in its [package]
# table and Cargo.lock in luna's own entry, the version the exe is built with.
# Each is read and written by the line that holds it, so a bump leaves the rest
# of the file, its line endings included, as it was.
$ReleaseVersionFiles = [ordered]@{
    'src-tauri/Cargo.toml' = 'cargo-toml'
    'src-tauri/Cargo.lock' = 'cargo-lock'
}

# The version files of the tree at $Root, in $ReleaseVersionFiles' order, or a
# throw naming the one that is missing.
function Get-ReleaseVersionFiles([string]$Root = $RepoRoot) {
    foreach ($file in $ReleaseVersionFiles.Keys) {
        if (-not (Test-Path -LiteralPath (Join-Path $Root $file) -PathType Leaf)) { throw "no $file in $Root; the release's version is written there" }
        $file
    }
}

# The version line of one of those files: the match of a pattern whose groups
# are the text before the version, the version and the text after it.
function Find-VersionLine([string]$Text, [string]$Kind) {
    $pattern = switch ($Kind) {
        # The first version key after the [package] header, before any other table.
        'cargo-toml' { '(?m)^(\[package\]\r?\n(?:(?!\[).*\n)*?version\s*=\s*")([^"]*)(")' }
        'cargo-lock' { '(?m)^(name = "luna"\r?\nversion = ")([^"]*)(")' }
    }
    [regex]::Match($Text, $pattern)
}

# Each version file's version, by its path from the root.
function Get-ReleaseVersions([string]$Root = $RepoRoot) {
    $found = [ordered]@{}
    foreach ($file in Get-ReleaseVersionFiles $Root) {
        $text = [IO.File]::ReadAllText((Join-Path $Root $file))
        $line = Find-VersionLine $text $ReleaseVersionFiles[$file]
        if (-not $line.Success) { throw "no version found in $file" }
        $found[$file] = $line.Groups[2].Value
    }
    $found
}

# The one version the version files agree on; throws, naming each, when they do not.
function Get-ReleaseVersion([string]$Root = $RepoRoot) {
    $found = Get-ReleaseVersions $Root
    $versions = @($found.Values | Sort-Object -Unique)
    if ($versions.Count -ne 1) {
        $each = ($found.Keys | ForEach-Object { "$_ $($found[$_])" }) -join ', '
        throw "the version files disagree: $each (./scripts/release.ps1 -Bump <version> sets them all)"
    }
    $versions[0]
}

# Semver as the updater compares it: x.y.z with an optional pre-release part.
function Assert-VersionFormat([string]$Version) {
    if ($Version -notmatch '^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$') {
        throw "'$Version' is not a version of the form 1.2.3 (or 1.2.3-rc.1)"
    }
}

# Writes $Version into every version file, each only on its version line.
function Set-ReleaseVersion([string]$Version, [string]$Root = $RepoRoot) {
    Assert-VersionFormat $Version
    # Every file is read and matched before any is written, so a file that
    # cannot be bumped leaves the others as they were.
    $edits = foreach ($file in Get-ReleaseVersionFiles $Root) {
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
$MarkdownLinkDefinition = '^ {0,3}\[(?<name>[^\]]+)\]:\s*(?<destination><[^>]*>|\S+)(?:\s+(?:"[^"]*"|''[^'']*''|\([^)]*\)))?\s*$'

# CHANGELOG.md read once: Sections, in the file's order, each with Version,
# Date, Heading (the line's text after "## "), Lines (its body as written, up
# to the next ## heading; the reference-link definitions taken out) and
# IntroEnd (how many of those lines come before its first ### subsection); and
# Links, every "[name]: url" definition from anywhere in the file, the line
# as written, by label lowercased, the first of a label kept, as CommonMark
# matches them. A heading, a definition or a ### inside a code fence is text,
# not structure ($MarkdownFenceOpen), so a `## heading` shown in a code block
# neither ends a section nor opens one. The release notes come from this
# file, so a tree without one cannot be released. Source line positions let
# the bump insert its section and compare link without re-rendering old notes.
function Read-Changelog([string]$Root) {
    $path = Join-Path $Root 'CHANGELOG.md'
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { throw "no CHANGELOG.md in $Root; the release notes are made from it" }
    $text = [IO.File]::ReadAllText($path)
    $lines = $text -split '\r?\n'
    $sections = [System.Collections.Generic.List[object]]::new()
    $links = @{}
    $linkLines = @{}
    $current = $null
    $fence = $null
    for ($i = 0; $i -lt $lines.Count; $i++) {
        $line = $lines[$i]
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
                Line     = $i
                Lines    = [System.Collections.Generic.List[string]]::new()
                IntroEnd = -1
            }
            $sections.Add($current)
            continue
        }
        if ($line -match '^## ') { $current = $null; continue }
        if ($line -match $MarkdownLinkDefinition) {
            $label = $Matches.name.ToLowerInvariant()
            if (-not $links.ContainsKey($label)) {
                $links[$label] = $line.Trim()
                $linkLines[$label] = $i
            }
            continue
        }
        if ($current) {
            if ($current.IntroEnd -lt 0 -and $line -match '^### ') { $current.IntroEnd = $current.Lines.Count }
            $current.Lines.Add($line)
        }
    }
    [pscustomobject]@{
        Sections = $sections
        Links = $links
        LinkLines = $linkLines
        SourceLines = $lines
        Newline = [regex]::Match($text, '\r?\n').Value
    }
}

$ChangeHeadings = 'Added', 'Changed', 'Deprecated', 'Removed', 'Fixed', 'Security'

function Read-ChangeFragment([IO.FileInfo]$File) {
    $from = "changes/$($File.Name)"
    if ($File.Name -cnotmatch '^[a-z0-9-]+\.md$' -or $File.LinkTarget) {
        throw "$($from): expected a regular file named with a lowercase slug (a-z, 0-9, hyphens) and .md"
    }
    $text = [IO.File]::ReadAllText($File.FullName) -creplace '^(?:[ \t]*\r?\n)+', ''
    $blocks = @($text -csplit '(?m)(?<=\n)(?=### )')
    $sections = foreach ($block in $blocks) {
        $heading, $body = $block -split '\r?\n', 2
        $kind = $heading.TrimEnd() -creplace '^### ', ''
        if ($heading.TrimEnd() -cne "### $kind") {
            throw "$($from): expected a ### changelog heading"
        }
        $body = ([string]$body).TrimEnd() -creplace '^(?:[ \t]*\r?\n)+', ''
        if ($kind -ceq 'None') {
            if ($blocks.Count -ne 1 -or $body -cnotmatch '^(?!#|- )\S[^\r\n]*$') {
                throw "$($from): ### None must stand alone and be followed by a non-empty reason line"
            }
            continue
        }
        if ($kind -cnotin $ChangeHeadings) {
            throw "$($from): unknown heading '$heading'; use $($ChangeHeadings -join ', ') or None"
        }
        $entryLines = $body -split '\r?\n'
        if ($entryLines[0] -cnotmatch '^- \S') {
            throw "$($from): $heading needs at least one bullet beginning with '- '"
        }
        foreach ($line in $entryLines) {
            if ($line.Trim() -and $line -cnotmatch '^(?:- \S| {2,}\S)') {
                throw "$($from): expected a bullet or an indented continuation under $heading"
            }
        }
        [pscustomobject]@{ Kind = $kind; Lines = $entryLines }
    }
    [pscustomobject]@{ Name = $File.Name; Path = $File.FullName; Sections = @($sections) }
}

function Get-ChangeFragmentFiles([string]$Root = $RepoRoot) {
    $directory = Join-Path $Root 'changes'
    if (-not (Test-Path -LiteralPath $directory)) { return }
    Get-ChildItem -LiteralPath $directory -File -Force |
        Where-Object { $_.Extension -ieq '.md' -and $_.Name -cne 'README.md' }
}

function Get-ChangeFragments([string]$Root = $RepoRoot) {
    [string[]]$paths = @(Get-ChangeFragmentFiles $Root | ForEach-Object FullName)
    [array]::Sort($paths, [StringComparer]::Ordinal)
    foreach ($path in $paths) { Read-ChangeFragment (Get-Item -LiteralPath $path -Force) }
}

function Assert-UnreleasedEmpty($Changelog) {
    $unreleased = @($Changelog.Sections | Where-Object { $_.Version -ieq 'Unreleased' })
    if ($unreleased.Count -ne 1 -or $Changelog.Sections[0].Heading -cne '[Unreleased]') {
        throw 'CHANGELOG.md must start its versions with one ## [Unreleased] section.'
    }
    if (($unreleased[0].Lines -join '').Trim()) {
        throw 'CHANGELOG.md must keep ## [Unreleased] empty. Put entries in changes/<slug>.md.'
    }
}

function New-ReleaseChangelog([string]$Version, [object[]]$Fragments, [string]$Root = $RepoRoot) {
    Assert-VersionFormat $Version
    $changelog = Read-Changelog $Root
    Assert-UnreleasedEmpty $changelog
    if (($changelog.Sections | Where-Object { $_.Version -eq $Version }) -or $changelog.Links.ContainsKey($Version)) {
        throw "CHANGELOG.md already has a section or link for $Version"
    }
    if ($changelog.Sections.Count -lt 2 -or -not $changelog.LinkLines.ContainsKey('unreleased')) {
        throw 'CHANGELOG.md needs the previous release section and the [Unreleased] compare link.'
    }
    $previous = $changelog.Sections[1].Version
    Assert-VersionFormat $previous
    $repo = Get-RepoUrl $Root
    foreach ($past in ($changelog.Sections | Select-Object -Skip 1)) {
        if (-not $changelog.Links.ContainsKey($past.Version)) {
            throw "CHANGELOG.md has no link definition for [$($past.Version)]."
        }
    }
    $expected = "$repo/compare/v$previous...HEAD"
    if ($changelog.Links['unreleased'] -notmatch $MarkdownLinkDefinition -or
        ($Matches.destination -creplace '^<(.*)>$', '$1') -cne $expected) {
        throw "CHANGELOG.md [Unreleased] must compare from v$previous to HEAD. Expected $expected."
    }
    $lines = [System.Collections.Generic.List[string]]::new([string[]]$changelog.SourceLines)
    $link = $changelog.LinkLines['unreleased']
    $lines[$link] = @(
        "[Unreleased]: $repo/compare/v$Version...HEAD"
        "[$($Version)]: $repo/compare/v$previous...v$Version"
    ) -join $changelog.Newline

    $date = (Get-Date).ToString('yyyy-MM-dd', [Globalization.CultureInfo]::InvariantCulture)
    $section = @(
        ''
        "## [$Version] - $date"
        foreach ($kind in $ChangeHeadings) {
            $groups = @($Fragments.Sections | Where-Object { $_.Kind -ceq $kind })
            if ($groups.Count -eq 0) { continue }
            ''
            "### $kind"
            ''
            foreach ($group in $groups) { $group.Lines }
        }
    )
    $lines.InsertRange($changelog.Sections[0].Line + 1, [string[]]$section)
    $lines -join $changelog.Newline
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

# The SBOM attested against the installer.
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
        docker run --rm --network none -v "${parent}:/parent" (Get-RustImage) rm -rf -- "/parent/$name"
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
    # By its exact name (build-windows.ps1's OUTFILE), not by whatever comes
    # first: the bundle folder keeps every installer ever built there, and
    # picking the first one alphabetically is how v0.2.0 went out carrying the
    # 0.1.0 setup and its signature, an update that offered a build its own
    # version. A folder an older build used also holds setups of 0.20's name,
    # Luna_<version>_x64-setup.exe, the same version's among them.
    $setup = Get-Item -LiteralPath (Join-Path $nsis "rm-luna_${Version}_x64-setup.exe") -ErrorAction SilentlyContinue
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

# The latest.json the updater fetches, in the form 0.19's plugin and 0.20's
# updater (desktop/updater.rs) both read: version, the core fingerprint (a
# window whose core has the same one installs the update by restarting only
# itself; 0.19 takes it from here, 0.20 from the signature's trusted comment,
# as this file is not signed), the notes the update chip shows (the CHANGELOG
# section's intro, Get-ChangelogSection), the date, and the installer with
# its signature, which the updater checks against the public key in
# src-tauri/updater.json. The installer is downloaded from the release the
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

# Where the updater's endpoint and public key are written, the one place:
# desktop/updater.rs compiles this file in, and the release scripts read it
# (Get-UpdaterEndpoint, Get-UpdaterPubkey), so a release is published where
# installed copies look and signed for the key they check with.
$UpdaterSource = 'src-tauri/updater.json'

# src-tauri/updater.json read: Endpoint and Pubkey. Throws unless it holds
# those two and nothing else, as desktop/updater.rs reads it.
function Read-UpdaterSource([string]$Root = $RepoRoot) {
    $path = Join-Path $Root $UpdaterSource
    $json = Get-Content -LiteralPath $path -Raw | ConvertFrom-Json -AsHashtable
    if ($json -isnot [System.Collections.IDictionary] -or (@($json.Keys | Sort-Object) -join ',') -cne 'endpoint,pubkey') {
        throw "$UpdaterSource should hold endpoint and pubkey, and nothing else"
    }
    foreach ($key in 'endpoint', 'pubkey') {
        if ($json[$key] -isnot [string] -or -not $json[$key]) { throw "$UpdaterSource has no $key" }
    }
    [pscustomobject]@{ Endpoint = $json.endpoint; Pubkey = $json.pubkey }
}

# The updater's public key, which every installed Luna checks an update's
# signature against: base64 of a minisign public key file.
function Get-UpdaterPubkey([string]$Root = $RepoRoot) {
    (Read-UpdaterSource $Root).Pubkey
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
    try { Read-UpdaterPubkey $Value | Out-Null } catch {
        throw "the trusted public key '$Value' is not a minisign public key as updater.json holds one (or `embedded`)"
    }
    $Value
}

# The official Rust image of the pinned toolchain, by tag and digest, the one
# the Linux check builds on (docker/linux-check.Dockerfile's FROM), pulled as
# it is: for what needs no more than a shell in a container (Remove-BuiltTree).
function Get-RustImage {
    $rust = @(Get-BaseImages | Where-Object { $_.File -eq 'docker/linux-check.Dockerfile' -and $_.Repo -eq 'rust' })
    if ($rust.Count -ne 1 -or -not $rust[0].Digest) {
        throw 'docker/linux-check.Dockerfile should have one FROM rust:<tag>@sha256:<digest> line'
    }
    $rust[0].Ref
}

# ---------------------------------------------------------------- signing
#
# The updater's signature on a release's installer is in the form Tauri's
# signer gave it up to 0.19, which every installed copy reads: base64 of a
# minisign signature file, prehashed (Ed25519 over the installer's
# BLAKE2b-512), with Tauri's untrusted comment and the trusted comment
# `timestamp:<unix>\tfile:<name>\tversion:<version>\tcore_fingerprint:<core>`.
# 0.19's tauri-plugin-updater reads `version:` and passes over the rest; 0.20's
# updater takes the core from there (desktop/updater.rs `verify`). The key is
# in the form `tauri signer generate` wrote it: base64 of a minisign secret
# key file, encrypted with an scrypt stream under a password that may be empty.
#
# Nothing but OpenSSL 3 from the system (Get-OpenSsl -ForKey) and the code
# below ever holds the key: no dependency's code, no build, no container.
# OpenSSL derives the scrypt stream that opens the key, makes its public half
# and signs, Ed25519 as RFC 8032 has it, which is deterministic. The code here
# reads the key file and checks its BLAKE2b checksum and its public half,
# composes the trusted comment at the moment of signing and the signature
# file, and holds each signature's S to the one RFC 8032 gives for the key and
# the message (Assert-Rfc8032Signature): a signer that picks its nonce can
# make valid signatures that hand the key to whoever knows the nonce, and
# none of those passes. The key reaches OpenSSL on its standard input, or in
# a file encrypted under a password made for the run (pkeyutl reads a key
# from a file only), that password by the name of a variable in OpenSSL's own
# environment; the key's own password goes on OpenSSL's standard input, as
# the UTF-8 bytes the Tauri CLI and minisign derive the key from
# (Get-ScryptStream). The release environment's key variables are kept out
# of every OpenSSL run (Invoke-OpenSsl, through which each goes), and the
# scripts here start no other process while they hold the key. Neither the
# key nor a password is ever on a command line, and no message carries
# anything of the key's or of what OpenSSL said with it: each is fixed text.

# OpenSSL 3. Where the updater's key is in play (-ForKey), only the one the
# system's own packages put in place: /usr/bin/openssl (on the release
# runner Ubuntu's, part of the runner image whose pwsh already holds the
# key), or on Windows one Git for Windows carries (Get-GitForWindowsOpenSsl),
# found where its installer registered it, not through PATH. To check a
# signature, an OpenSSL 3 on PATH does too. It has to be OpenSSL 3 (scrypt,
# BLAKE2b-512, Ed25519 over raw input): macOS's own `openssl`, LibreSSL, is
# not taken.
function Get-OpenSsl([switch]$ForKey) {
    $candidates = @(
        if ($IsWindows) { Get-GitForWindowsOpenSsl } else { '/usr/bin/openssl' }
        if (-not $ForKey) { Get-Command openssl -CommandType Application -All -ErrorAction SilentlyContinue | ForEach-Object Source }
    )
    foreach ($openssl in $candidates) {
        if (-not (Test-Path -LiteralPath $openssl -PathType Leaf)) { continue }
        $version = try { [Text.Encoding]::ASCII.GetString((Invoke-OpenSsl $openssl 'asking its version' @('version'))) } catch { '' }
        if ($version -cmatch '^OpenSSL 3\.') { return $openssl }
    }
    if ($ForKey) {
        throw 'signing takes the system''s OpenSSL 3: /usr/bin/openssl, or on Windows the one Git for Windows carries, installed for all users (its InstallPath under HKLM\SOFTWARE\GitForWindows); none was found'
    }
    throw 'checking an updater signature takes OpenSSL 3 (openssl on PATH, or the one Git for Windows carries); none was found'
}

# The OpenSSL builds Git for Windows carries, by the folder its installer
# wrote under HKLM\SOFTWARE\GitForWindows (InstallPath), in the 64-bit
# registry view and then the 32-bit one: a key only an administrator can
# change, where the git found first on PATH, or the folder it names, is
# whatever a user-writable folder early on PATH holds. Its environment build
# (ucrt64, or mingw64 and clangarm64 in other releases) first, then the
# MSYS2 one under usr. Starts no process: nothing that runs while the key is
# held sees it.
function Get-GitForWindowsOpenSsl {
    foreach ($view in [Microsoft.Win32.RegistryView]::Registry64, [Microsoft.Win32.RegistryView]::Registry32) {
        $base = [Microsoft.Win32.RegistryKey]::OpenBaseKey([Microsoft.Win32.RegistryHive]::LocalMachine, $view)
        try {
            $key = $base.OpenSubKey('SOFTWARE\GitForWindows')
            $install = if ($key) { try { $key.GetValue('InstallPath') -as [string] } finally { $key.Dispose() } }
        } finally {
            $base.Dispose()
        }
        if (-not $install -or -not [IO.Path]::IsPathFullyQualified($install)) { continue }
        foreach ($dir in 'ucrt64', 'mingw64', 'clangarm64', 'usr') { Join-Path $install "$dir/bin/openssl.exe" }
    }
}

# Runs $OpenSsl with $Arguments, each passed as it is (no shell between),
# $Stdin on its standard input and $Environment added to its own environment
# alone, the variables the release environment hands the key and its password
# in taken out of it: OpenSSL gets the key the way the call gives it, no
# other. The only process the signing starts. Returns what it wrote to its standard output, as bytes. What it says
# on its standard error, and its exit code, are dropped unread: with a key in
# play either could say something of it. A failure throws "$What failed
# (OpenSSL)".
function Invoke-OpenSsl {
    param(
        [Parameter(Mandatory)][string]$OpenSsl,
        [Parameter(Mandatory)][string]$What,
        [Parameter(Mandatory)][string[]]$Arguments,
        [byte[]]$Stdin = @(),
        [hashtable]$Environment = @{}
    )
    $start = [Diagnostics.ProcessStartInfo]::new($OpenSsl)
    foreach ($argument in $Arguments) { $start.ArgumentList.Add($argument) }
    foreach ($name in 'TAURI_SIGNING_PRIVATE_KEY', 'TAURI_SIGNING_PRIVATE_KEY_PASSWORD') { [void]$start.Environment.Remove($name) }
    foreach ($name in $Environment.Keys) { $start.Environment[$name] = $Environment[$name] }
    $start.UseShellExecute = $false
    $start.RedirectStandardInput = $true
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    $process = [Diagnostics.Process]::Start($start)
    try {
        $out = [IO.MemoryStream]::new()
        $reading = $process.StandardOutput.BaseStream.CopyToAsync($out)
        $dropping = $process.StandardError.BaseStream.CopyToAsync([IO.Stream]::Null)
        # A command that reads nothing may be gone before its input is
        # written; its exit code says how it went.
        try { $process.StandardInput.BaseStream.Write($Stdin, 0, $Stdin.Length) } catch [IO.IOException] {}
        try { $process.StandardInput.Close() } catch [IO.IOException] {}
        $reading.Wait()
        $dropping.Wait()
        $process.WaitForExit()
        if ($process.ExitCode) { throw "$What failed (OpenSSL)" }
        , $out.ToArray()
    } finally {
        $process.Dispose()
    }
}

# Whether two byte arrays hold the same bytes.
function Test-SameBytes([byte[]]$A, [byte[]]$B) {
    if ($A.Length -ne $B.Length) { return $false }
    $diff = 0
    for ($i = 0; $i -lt $A.Length; $i++) { $diff = $diff -bor ($A[$i] -bxor $B[$i]) }
    $diff -eq 0
}

# BLAKE2b (RFC 7693), unkeyed, $Length bytes long, of $Data: the checksum in a
# minisign secret key is BLAKE2b-256, which OpenSSL 3.0, the release
# runner's, does not make (its BLAKE2b is 512 bits only). For a few bytes in
# memory: it runs at PowerShell's pace. Its sums go through Add-UInt64, as
# PowerShell turns a [uint64] sum that overflows into a double.
$Blake2bIv = [uint64[]]@(
    0x6a09e667f3bcc908ul, 0xbb67ae8584caa73bul, 0x3c6ef372fe94f82bul, 0xa54ff53a5f1d36f1ul,
    0x510e527fade682d1ul, 0x9b05688c2b3e6c1ful, 0x1f83d9abfb41bd6bul, 0x5be0cd19137e2179ul
)
$Blake2bSigma = @(
    @(0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15), @(14, 10, 4, 8, 9, 15, 13, 6, 1, 12, 0, 2, 11, 7, 5, 3),
    @(11, 8, 12, 0, 5, 2, 15, 13, 10, 14, 3, 6, 7, 1, 9, 4), @(7, 9, 3, 1, 13, 12, 11, 14, 2, 6, 5, 10, 4, 0, 15, 8),
    @(9, 0, 5, 7, 2, 4, 10, 15, 14, 1, 11, 12, 6, 8, 3, 13), @(2, 12, 6, 10, 0, 11, 8, 3, 4, 13, 7, 5, 15, 14, 1, 9),
    @(12, 5, 1, 15, 14, 13, 4, 10, 0, 7, 6, 3, 9, 2, 8, 11), @(13, 11, 7, 14, 12, 1, 3, 9, 5, 0, 15, 4, 8, 6, 2, 10),
    @(6, 15, 14, 9, 11, 3, 0, 8, 12, 2, 13, 7, 1, 4, 10, 5), @(10, 2, 8, 4, 7, 6, 1, 5, 15, 11, 9, 14, 3, 12, 13, 0)
)
# The words each of a round's eight mixes takes: four columns, four diagonals.
$Blake2bMixes = @(@(0, 4, 8, 12), @(1, 5, 9, 13), @(2, 6, 10, 14), @(3, 7, 11, 15), @(0, 5, 10, 15), @(1, 6, 11, 12), @(2, 7, 8, 13), @(3, 4, 9, 14))

function Add-UInt64([uint64]$A, [uint64]$B) {
    $low = ($A -band 0xFFFFFFFFul) + ($B -band 0xFFFFFFFFul)
    $high = (($A -shr 32) + ($B -shr 32) + ($low -shr 32)) -band 0xFFFFFFFFul
    ($high -shl 32) -bor ($low -band 0xFFFFFFFFul)
}

function Get-Blake2b([byte[]]$Data, [int]$Length = 64) {
    if ($Length -lt 1 -or $Length -gt 64) { throw "BLAKE2b gives 1 to 64 bytes, not $Length" }
    $h = [uint64[]]$Blake2bIv.Clone()
    $h[0] = $h[0] -bxor [uint64](0x01010000 -bor $Length)
    $blocks = [math]::Max(1, [math]::Ceiling($Data.Length / 128))
    for ($n = 0; $n -lt $blocks; $n++) {
        $block = [byte[]]::new(128)
        $take = [math]::Min(128, $Data.Length - 128 * $n)
        if ($take -gt 0) { [Array]::Copy($Data, 128 * $n, $block, 0, $take) }
        $m = [uint64[]]::new(16)
        for ($i = 0; $i -lt 16; $i++) { $m[$i] = [BitConverter]::ToUInt64($block, 8 * $i) }
        $v = [uint64[]]::new(16)
        for ($i = 0; $i -lt 8; $i++) { $v[$i] = $h[$i]; $v[$i + 8] = $Blake2bIv[$i] }
        $v[12] = $v[12] -bxor [uint64](128 * $n + [math]::Max($take, 0))
        if ($n -eq $blocks - 1) { $v[14] = $v[14] -bxor [uint64]::MaxValue }
        for ($round = 0; $round -lt 12; $round++) {
            $s = $Blake2bSigma[$round % 10]
            for ($j = 0; $j -lt 8; $j++) {
                $a, $b, $c, $d = $Blake2bMixes[$j]
                $v[$a] = Add-UInt64 (Add-UInt64 $v[$a] $v[$b]) $m[$s[2 * $j]]
                $x = $v[$d] -bxor $v[$a]; $v[$d] = ($x -shr 32) -bor ($x -shl 32)
                $v[$c] = Add-UInt64 $v[$c] $v[$d]
                $x = $v[$b] -bxor $v[$c]; $v[$b] = ($x -shr 24) -bor ($x -shl 40)
                $v[$a] = Add-UInt64 (Add-UInt64 $v[$a] $v[$b]) $m[$s[2 * $j + 1]]
                $x = $v[$d] -bxor $v[$a]; $v[$d] = ($x -shr 16) -bor ($x -shl 48)
                $v[$c] = Add-UInt64 $v[$c] $v[$d]
                $x = $v[$b] -bxor $v[$c]; $v[$b] = ($x -shr 63) -bor ($x -shl 1)
            }
        }
        for ($i = 0; $i -lt 8; $i++) { $h[$i] = $h[$i] -bxor $v[$i] -bxor $v[$i + 8] }
    }
    $out = [byte[]]::new(64)
    for ($i = 0; $i -lt 8; $i++) { [Array]::Copy([BitConverter]::GetBytes($h[$i]), 0, $out, 8 * $i, 8) }
    , [byte[]]$out[0..($Length - 1)]
}

# The scrypt parameters a minisign secret key's opslimit and memlimit stand
# for, worked out as libsodium's pickparams does, which minisign 0.10, the
# crate the Tauri CLI opens keys with, follows (helpers.rs
# raw_scrypt_params), within minisign's bounds: memlimit up to 1 GiB, N up to
# 2^20. A key the Tauri CLI or minisign made has 2^20 and 32 MiB, so N = 2^15,
# r = 8, p = 1. On purpose, this takes no more than such keys need: a p above
# 64, which scrypt and minisign would take (N = 2^10, r = 8, p = 65, say), is
# refused, and the message says so. Where the two orders differ, the cap on
# r·p (2^30 - 1) is libsodium's, taken before the narrowing to 32 bits that
# minisign makes first: they part only where (opslimit / 4) / N is past 2^32,
# where libsodium's p is far over 64, so a key they would open two ways is
# refused rather than opened one of them. Returns LogN, R and P.
function Get-MinisignScryptParams([uint64]$Opslimit, [uint64]$Memlimit) {
    $unusable = 'the updater key''s scrypt parameters are not ones the release scripts open: they take memlimit up to 1 GiB, N up to 2^20 and p up to 64, as every key `tauri signer generate` or minisign makes has (N = 2^15, r = 8, p = 1); a key made with other limits is refused on purpose'
    if ($Memlimit -gt 1073741824) { throw $unusable }
    $ops = if ($Opslimit -lt 32768) { [uint64]32768 } else { $Opslimit }
    $log = 1
    if ($ops -lt ($Memlimit -shr 5)) {
        $p = 1
        $maxN = $ops -shr 5
        while ($log -lt 63 -and ([uint64]1 -shl $log) -le ($maxN -shr 1)) { $log++ }
    } else {
        $maxN = $Memlimit -shr 10
        while ($log -lt 63 -and ([uint64]1 -shl $log) -le ($maxN -shr 1)) { $log++ }
        # libsodium bounds (opslimit / 4) / N first, then takes it as 32 bits.
        $maxRp = ($ops -shr 2) -shr $log
        if ($maxRp -gt 0x3FFFFFFF) { $maxRp = [uint64]0x3FFFFFFF }
        $p = $maxRp -shr 3
    }
    if ($log -gt 20 -or $p -lt 1 -or $p -gt 64) { throw $unusable }
    [pscustomobject]@{ LogN = [int]$log; R = 8; P = [int]$p }
}

# scrypt's 104-byte stream for $Password and $Salt under $Params
# (Get-MinisignScryptParams), from OpenSSL: the bytes a minisign secret key's
# key id, key and checksum are XORed with. The password is scrypt's input as
# its UTF-8 bytes, as minisign 0.10 takes it (`password.as_bytes()`), from
# the Tauri CLI as from its own prompt. It goes on OpenSSL's standard input
# (`pass:stdin`), those bytes as they are, ended by a newline, which OpenSSL
# reads up to; never on its command line, nor by an environment variable,
# which Git for Windows' OpenSSL reads through the ANSI code page, so a
# password beyond ASCII would not open the key there. What OpenSSL cannot
# take that way (a line break or another control character, which its read
# would cut at or, on Windows, take for the end, or more than 1023 bytes,
# past which it reads no further) is refused. An empty password, which
# says nothing, goes as an empty option.
function Get-ScryptStream([string]$OpenSsl, [byte[]]$Salt, $Params, [AllowEmptyString()][string]$Password) {
    $n = [uint64]1 -shl $Params.LogN
    # What OpenSSL's scrypt allocates (kdf_scrypt.c: B, then V), allowed it.
    $memory = [uint64](128 * $Params.R) * ($n + 2) + [uint64](128 * $Params.R * $Params.P)
    $arguments = @('pkeyutl', '-kdf', 'scrypt', '-kdflen', '104', '-pkeyopt', "hexsalt:$([Convert]::ToHexString($Salt))",
        '-pkeyopt', "N:$n", '-pkeyopt', "r:$($Params.R)", '-pkeyopt', "p:$($Params.P)", '-pkeyopt', "maxmem_bytes:$memory")
    $stdin = [byte[]]@()
    if ($Password) {
        if ($Password -match '\p{Cc}') { throw 'the updater key''s password has a line break or another control character, which OpenSSL cannot be handed on its input' }
        $stdin = try { [Text.UTF8Encoding]::new($false, $true).GetBytes("$Password`n") } catch { throw 'the updater key''s password is not valid Unicode' }
        if ($stdin.Length -gt 1024) {
            [Array]::Clear($stdin)
            throw 'the updater key''s password is longer than the 1023 bytes OpenSSL reads of one'
        }
        $arguments += '-pkeyopt_passin', 'pass:stdin'
    } else {
        $arguments += '-pkeyopt', 'pass:'
    }
    try {
        $stream = Invoke-OpenSsl $OpenSsl 'opening the updater key' $arguments -Stdin $stdin
    } finally {
        [Array]::Clear($stdin)
    }
    if ($stream.Length -ne 104) { throw 'opening the updater key failed (OpenSSL)' }
    , $stream
}

# An Ed25519 key's PKCS#8 and a public key's SubjectPublicKeyInfo (RFC 8410):
# fixed DER, then the 32 bytes of the seed or of the public key.
$Ed25519Pkcs8Prefix = [byte[]](0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04, 0x20)
$Ed25519SpkiPrefix = [byte[]](0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00)

# The public key of the Ed25519 key with the seed $Seed, made by OpenSSL, the
# key on its standard input.
function Get-Ed25519Public([string]$OpenSsl, [byte[]]$Seed) {
    $pkcs8 = [byte[]]($Ed25519Pkcs8Prefix + $Seed)
    try {
        $spki = Invoke-OpenSsl $OpenSsl 'making the key''s public half' @('pkey', '-inform', 'DER', '-pubout', '-outform', 'DER') -Stdin $pkcs8
    } finally {
        [Array]::Clear($pkcs8)
    }
    if ($spki.Length -ne 44 -or -not (Test-SameBytes ([byte[]]$spki[0..11]) $Ed25519SpkiPrefix)) { throw 'OpenSSL made no Ed25519 public key' }
    , [byte[]]$spki[12..43]
}

# The untrusted comment Tauri's signer gives every signature. Nothing reads
# it; it stays as it was so the .sig is in the form Tauri's signer made.
$UpdaterUntrustedComment = 'signature from tauri secret key'

# The updater key $Key (the text of the file `tauri signer generate` wrote, or
# the release environment's secret: base64 of a minisign secret key file)
# opened with $Password, empty for a key made without one, as the
# maintainer's is. The file, as minisign 0.10 writes it (secret_key.rs): an
# untrusted comment, then base64 of "Ed", "Sc" (scrypt), "B2" (BLAKE2b), the
# salt, opslimit and memlimit, and 104 bytes XORed with scrypt's stream: the
# key id, the Ed25519 key (its seed, then its public key) and a BLAKE2b-256
# checksum of "Ed", the key id and the key. A wrong password fails the
# checksum, and the public key the file carries has to be the one OpenSSL
# makes of the seed. Returns KeyId, Seed and Public (8, 32 and 32 bytes).
function Read-UpdaterSecretKey([string]$OpenSsl, [string]$Key, [AllowEmptyString()][string]$Password) {
    $bin = [byte[]]$(
        try {
            $lines = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($Key.Trim())) -split '\r?\n'
            if ($lines.Count -ge 2 -and $lines[0].StartsWith('untrusted comment: ')) { [Convert]::FromBase64String($lines[1]) }
        } catch {}
    )
    if (-not $bin -or $bin.Length -ne 158 -or [Text.Encoding]::ASCII.GetString($bin, 0, 6) -cne 'EdScB2') {
        throw 'the updater key is not a secret key in the form `tauri signer generate` writes'
    }
    $params = Get-MinisignScryptParams ([BitConverter]::ToUInt64($bin, 38)) ([BitConverter]::ToUInt64($bin, 46))
    $stream = Get-ScryptStream $OpenSsl ([byte[]]$bin[6..37]) $params $Password
    $plain = [byte[]]::new(104)
    for ($i = 0; $i -lt 104; $i++) { $plain[$i] = $bin[54 + $i] -bxor $stream[$i] }
    [Array]::Clear($stream)
    $opened = [pscustomobject]@{ KeyId = [byte[]]$plain[0..7]; Seed = [byte[]]$plain[8..39]; Public = [byte[]]$plain[40..71] }
    $checksum = [byte[]]$plain[72..103]
    [Array]::Clear($plain)
    $summed = [byte[]]([byte[]](0x45, 0x64) + $opened.KeyId + $opened.Seed + $opened.Public)
    $expected = Get-Blake2b $summed 32
    [Array]::Clear($summed)
    if (-not (Test-SameBytes $expected $checksum)) {
        [Array]::Clear($opened.Seed)
        throw 'the updater key does not open with the password given'
    }
    if (-not (Test-SameBytes (Get-Ed25519Public $OpenSsl $opened.Seed) $opened.Public)) {
        [Array]::Clear($opened.Seed)
        throw 'the updater key is damaged: the public key it carries is not its own'
    }
    $opened
}

# The key id and Ed25519 public key of $Pubkey, base64 of a minisign public
# key file as updater.json holds one: KeyId and Public, 8 and 32 bytes.
function Read-UpdaterPubkey([string]$Pubkey) {
    $bin = [byte[]]$(
        try {
            $lines = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($Pubkey.Trim())) -split '\r?\n'
            if ($lines.Count -ge 2) { [Convert]::FromBase64String($lines[1]) }
        } catch {}
    )
    if (-not $bin -or $bin.Length -ne 42 -or $bin[0] -ne 0x45 -or $bin[1] -ne 0x64) { throw 'the public key is not a minisign public key as updater.json holds one' }
    [pscustomobject]@{ KeyId = [byte[]]$bin[2..9]; Public = [byte[]]$bin[10..41] }
}

# A new updater key pair in the form `tauri signer generate --password ''`
# wrote one, with minisign 0.10's defaults (opslimit 2^20, memlimit 32 MiB):
# the Ed25519 key made by OpenSSL, the key id and the salt random, the key
# encrypted with the scrypt stream of an empty password. Returns Secret, the
# key file's text, and Public, the public key as updater.json holds one; it
# writes nothing.
function New-UpdaterKeyPair([string]$OpenSsl) {
    $pkcs8 = Invoke-OpenSsl $OpenSsl 'making a key' @('genpkey', '-algorithm', 'ed25519', '-outform', 'DER')
    if ($pkcs8.Length -ne 48 -or -not (Test-SameBytes ([byte[]]$pkcs8[0..15]) $Ed25519Pkcs8Prefix)) { throw 'OpenSSL made no Ed25519 key' }
    $seed = [byte[]]$pkcs8[16..47]
    [Array]::Clear($pkcs8)
    $public = Get-Ed25519Public $OpenSsl $seed
    $keyId = [Security.Cryptography.RandomNumberGenerator]::GetBytes(8)
    $salt = [Security.Cryptography.RandomNumberGenerator]::GetBytes(32)
    $opslimit = [uint64]1048576
    $memlimit = [uint64]33554432
    $summed = [byte[]]([byte[]](0x45, 0x64) + $keyId + $seed + $public)
    $plain = [byte[]]($keyId + $seed + $public + (Get-Blake2b $summed 32))
    [Array]::Clear($seed)
    [Array]::Clear($summed)
    $stream = Get-ScryptStream $OpenSsl $salt (Get-MinisignScryptParams $opslimit $memlimit) ''
    for ($i = 0; $i -lt 104; $i++) { $plain[$i] = $plain[$i] -bxor $stream[$i] }
    $bin = [byte[]]([Text.Encoding]::ASCII.GetBytes('EdScB2') + $salt + [BitConverter]::GetBytes($opslimit) + [BitConverter]::GetBytes($memlimit) + $plain)
    $secretFile = "untrusted comment: rsign encrypted secret key`n$([Convert]::ToBase64String($bin))`n"
    # minisign names a key by the little-endian number its id's 8 bytes are.
    $publicFile = "untrusted comment: minisign public key: $([BitConverter]::ToUInt64($keyId, 0).ToString('X16'))`n" +
        "$([Convert]::ToBase64String([byte[]]([byte[]](0x45, 0x64) + $keyId + $public)))`n"
    [pscustomobject]@{
        Secret = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($secretFile))
        Public = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($publicFile))
    }
}

# Ed25519's group order (RFC 8032, 5.1): 2^252 + 27742317777372353535851937790883648493.
$Ed25519Order = [System.Numerics.BigInteger]::Pow(2, 252) + [System.Numerics.BigInteger]::Parse('27742317777372353535851937790883648493')

# The number $Bytes are, little-endian, unsigned.
function ConvertFrom-LittleEndian([byte[]]$Bytes) {
    [System.Numerics.BigInteger]::new([byte[]]($Bytes + [byte]0))
}

# Throws unless $Signature (64 bytes: R, then S) is the Ed25519 signature RFC
# 8032 (5.1.6) makes of $Message with the key whose seed is $Seed and public
# key $Public: S = r + k·s mod L, where s is the seed's clamped scalar, r =
# SHA-512(prefix ‖ M) and k = SHA-512(R ‖ A ‖ M), both mod L. With OpenSSL's
# check that S·B = R + k·A under that A (Assert-UpdaterSignatureVerifies),
# this holds R to r·B: the nonce is the one RFC 8032 derives, not one anyone
# chose, so R and S tell nothing of the key; and the signature is the one
# deterministic signing makes, the same every time.
function Assert-Rfc8032Signature([byte[]]$Seed, [byte[]]$Public, [byte[]]$Message, [byte[]]$Signature) {
    $h = [Security.Cryptography.SHA512]::HashData($Seed)
    try {
        $scalar = [byte[]]$h[0..31]
        $scalar[0] = $scalar[0] -band 248
        $scalar[31] = ($scalar[31] -band 127) -bor 64
        $r = (ConvertFrom-LittleEndian ([Security.Cryptography.SHA512]::HashData([byte[]]([byte[]]$h[32..63] + $Message)))) % $Ed25519Order
        $k = (ConvertFrom-LittleEndian ([Security.Cryptography.SHA512]::HashData([byte[]]([byte[]]$Signature[0..31] + $Public + $Message)))) % $Ed25519Order
        $s = ($r + $k * (ConvertFrom-LittleEndian $scalar)) % $Ed25519Order
        [Array]::Clear($scalar)
        if ($s -ne (ConvertFrom-LittleEndian ([byte[]]$Signature[32..63]))) {
            throw 'a signature is not the one RFC 8032 makes for this key and message; it is not used'
        }
    } finally {
        [Array]::Clear($h)
    }
}

# The .sig of the installer at $Path for the updater (base64 of a minisign
# signature file, in the form Tauri's signer wrote it) with $Key
# (Read-UpdaterSecretKey's), naming the installer, $Version and the core
# $Fingerprint, dated by $Now as the trusted comment is signed. OpenSSL hashes
# the installer and signs, the key handed to it in PKCS#8 encrypted under a
# password made for this call (`pkey -aes-256-cbc`, the key on its input),
# written under $Work for `pkeyutl -sign`, which reads a key only from a
# file, and deleted once both signatures are made; the password is in the
# environment of those runs alone. Each signature is held to RFC 8032
# (Assert-Rfc8032Signature). Returns Signature, the .sig's text, and
# Timestamp, the date in its trusted comment.
function New-UpdaterSignature {
    param(
        [Parameter(Mandatory)][string]$OpenSsl,
        [Parameter(Mandatory)]$Key,
        [Parameter(Mandatory)][string]$Path,
        [Parameter(Mandatory)][string]$Version,
        [Parameter(Mandatory)][string]$Fingerprint,
        [Parameter(Mandatory)][string]$Work,
        [scriptblock]$Now = { [DateTimeOffset]::UtcNow }
    )
    Assert-VersionFormat $Version
    Assert-CoreFingerprint $Fingerprint 'the core to sign for'
    $name = Split-Path -Leaf $Path
    if ($name -cnotmatch '\A[A-Za-z0-9._+-]+\z') { throw "'$name' is not an installer name a trusted comment carries" }
    $transit = @{ LUNA_SIGNING_KEY_PASSWORD = [Convert]::ToHexString([Security.Cryptography.RandomNumberGenerator]::GetBytes(32)) }
    $keyFile = Join-Path $Work 'signing-key.pem'
    $pkcs8 = [byte[]]($Ed25519Pkcs8Prefix + $Key.Seed)
    try {
        $pem = Invoke-OpenSsl $OpenSsl 'handing the key to OpenSSL' @('pkey', '-inform', 'DER', '-outform', 'PEM', '-aes-256-cbc', '-passout', 'env:LUNA_SIGNING_KEY_PASSWORD') `
            -Stdin $pkcs8 -Environment $transit
        [IO.File]::WriteAllBytes($keyFile, $pem)
        $sign = {
            param([byte[]]$Message, [string]$As)
            $in = Join-Path $Work $As
            [IO.File]::WriteAllBytes($in, $Message)
            $made = Invoke-OpenSsl $OpenSsl 'signing' @('pkeyutl', '-sign', '-rawin', '-inkey', $keyFile, '-passin', 'env:LUNA_SIGNING_KEY_PASSWORD', '-in', $in) -Environment $transit
            if ($made.Length -ne 64) { throw 'signing failed (OpenSSL)' }
            Assert-Rfc8032Signature $Key.Seed $Key.Public $Message $made
            , $made
        }
        $hash = Invoke-OpenSsl $OpenSsl 'hashing the installer' @('dgst', '-blake2b512', '-binary', $Path)
        if ($hash.Length -ne 64) { throw 'hashing the installer failed (OpenSSL)' }
        $signature = & $sign $hash 'installer.blake2b'
        # Dated as it is signed, whatever came before took (the key opened,
        # the installer hashed).
        $timestamp = (& $Now).ToUnixTimeSeconds()
        $comment = "timestamp:$timestamp`tfile:$name`tversion:$Version`tcore_fingerprint:$Fingerprint"
        $global = & $sign ([byte[]]($signature + [Text.Encoding]::ASCII.GetBytes($comment))) 'trusted-comment.bin'
    } finally {
        [Array]::Clear($pkcs8)
        Remove-Item -LiteralPath $keyFile -Force -ErrorAction SilentlyContinue
    }
    $bin = [byte[]]([Text.Encoding]::ASCII.GetBytes('ED') + $Key.KeyId + $signature)
    $file = "untrusted comment: $UpdaterUntrustedComment`n$([Convert]::ToBase64String($bin))`ntrusted comment: $comment`n$([Convert]::ToBase64String($global))`n"
    [pscustomobject]@{ Signature = [Convert]::ToBase64String([Text.Encoding]::ASCII.GetBytes($file)); Timestamp = $timestamp }
}

# Makes a new updater key pair (New-UpdaterKeyPair) for a key rotation
# (docs/releasing.md): $Path, the secret key in the Tauri CLI's form with an
# empty password, and $Path.pub, its public half as updater.json holds one;
# neither may be there yet. $Path is made readable by this user alone from
# its creation: mode 600 where the system has file modes, and on Windows a
# protected DACL, nothing inherited from the folder, that lets in this user,
# SYSTEM and Administrators alone, as Luna's own private files have
# (secret.rs `private_sddl`). The key is read back as a release reads it
# before either file is written. Returns the public key.
function New-UpdaterKey([string]$Path) {
    $Path = [IO.Path]::GetFullPath($Path, (Get-Location).Path)
    foreach ($file in $Path, "$Path.pub") { if (Test-Path -LiteralPath $file) { throw "$file is there already" } }
    $openssl = Get-OpenSsl -ForKey
    $pair = New-UpdaterKeyPair $openssl
    $read = Read-UpdaterSecretKey $openssl $pair.Secret ''
    [Array]::Clear($read.Seed)
    if (-not (Test-SameBytes $read.Public (Read-UpdaterPubkey $pair.Public).Public)) { throw 'the key made does not read back as its public key says' }
    New-Item -ItemType Directory -Force (Split-Path -Parent $Path) | Out-Null
    $stream = New-PrivateFile $Path
    try {
        $bytes = [Text.Encoding]::ASCII.GetBytes($pair.Secret)
        $stream.Write($bytes, 0, $bytes.Length)
    } finally {
        $stream.Dispose()
    }
    [IO.File]::WriteAllText("$Path.pub", $pair.Public, [Text.UTF8Encoding]::new($false))
    Write-Host "made a key pair: $Path and $Path.pub"
    $pair.Public
}

# The DACL, as SDDL, of a file of this user's alone: protected (nothing
# inherited), full control for the user, SYSTEM and Administrators, by SID,
# as secret.rs `private_sddl` gives Luna's own private files.
function Get-PrivateSddl {
    "D:P(A;;FA;;;$([Security.Principal.WindowsIdentity]::GetCurrent().User.Value))(A;;FA;;;SY)(A;;FA;;;BA)"
}

# A new file at $Path, open for writing, that nobody but this user may read
# from the moment it is made: mode 600 where the system has file modes, on
# Windows the DACL of Get-PrivateSddl set as it is created. A Windows volume
# that keeps no file permissions (FAT, exFAT) would drop that DACL without a
# word, so its folder is refused there first (Test-PersistentAcls). Throws
# where $Path is there already.
function New-PrivateFile([string]$Path) {
    if (-not $IsWindows) {
        $options = [IO.FileStreamOptions]@{ Mode = [IO.FileMode]::CreateNew; Access = [IO.FileAccess]::Write }
        $options.UnixCreateMode = [IO.UnixFileMode]'UserRead, UserWrite'
        return [IO.FileStream]::new($Path, $options)
    }
    if (-not (Test-PersistentAcls (Split-Path -Parent $Path))) {
        throw 'the key cannot be kept private there: its volume keeps no file permissions (FAT or exFAT, say); make it in a folder on an NTFS or ReFS volume'
    }
    $security = [Security.AccessControl.FileSecurity]::new()
    $security.SetSecurityDescriptorSddlForm((Get-PrivateSddl), [Security.AccessControl.AccessControlSections]::Access)
    [IO.FileSystemAclExtensions]::Create([IO.FileInfo]::new($Path), [IO.FileMode]::CreateNew, [Security.AccessControl.FileSystemRights]::Write,
        [IO.FileShare]::None, 4096, [IO.FileOptions]::None, $security)
}

# Whether the Windows volume the existing folder $Dir is on keeps a file's
# permissions: the FILE_PERSISTENT_ACLS flag GetVolumeInformationW gives its
# volume, found by GetVolumePathNameW, so a volume mounted in a folder is
# the one asked. NTFS and ReFS keep them; FAT and exFAT do not. False where
# either call fails.
function Test-PersistentAcls([string]$Dir) {
    if (-not ('LunaRelease.Volume' -as [type])) {
        Add-Type -Namespace LunaRelease -Name Volume -MemberDefinition @'
[DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
public static extern bool GetVolumePathNameW(string fileName, System.Text.StringBuilder volumePathName, uint bufferLength);
[DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
public static extern bool GetVolumeInformationW(string rootPathName, System.Text.StringBuilder volumeNameBuffer, uint volumeNameSize,
    out uint volumeSerialNumber, out uint maximumComponentLength, out uint fileSystemFlags, System.Text.StringBuilder fileSystemNameBuffer, uint fileSystemNameSize);
'@
    }
    $root = [Text.StringBuilder]::new(32768)
    if (-not [LunaRelease.Volume]::GetVolumePathNameW([IO.Path]::GetFullPath($Dir), $root, [uint32]$root.Capacity)) { return $false }
    [uint32]$serial = 0; [uint32]$longest = 0; [uint32]$flags = 0
    if (-not [LunaRelease.Volume]::GetVolumeInformationW($root.ToString(), $null, 0, [ref]$serial, [ref]$longest, [ref]$flags, $null, 0)) { return $false }
    # FILE_PERSISTENT_ACLS
    ($flags -band 0x8) -ne 0
}

# Throws unless $Signature (a .sig's text) is in exactly the form
# New-UpdaterSignature writes for the installer $File of $Version, built with
# the core $Fingerprint, under the key $Pubkey, and nothing else: base64 with
# nothing around it of a minisign signature file in ASCII, its four lines
# ended by LF; Tauri's untrusted comment; a prehashed Ed25519 signature
# ("ED") under $Pubkey's key id; the trusted comment exactly
# `timestamp:<1 to 10 ASCII digits>`, `file:$File`, `version:$Version`,
# `core_fingerprint:$Fingerprint`, tab-separated; a 64-byte global signature;
# each base64 line canonical. With -Timestamp, the date is that one (the
# signing's own); without, not more than a day ahead of this clock. Its
# messages are fixed text, never a piece of the signature.
function Assert-UpdaterSignatureForm {
    param(
        [Parameter(Mandatory)][AllowEmptyString()][string]$Signature,
        [Parameter(Mandatory)][string]$Pubkey,
        [Parameter(Mandatory)][string]$File,
        [Parameter(Mandatory)][string]$Version,
        [Parameter(Mandatory)][string]$Fingerprint,
        [Nullable[long]]$Timestamp
    )
    Assert-VersionFormat $Version
    Assert-CoreFingerprint $Fingerprint 'the expected core'
    if ($File -cnotmatch '\A[A-Za-z0-9._+-]+\z') { throw "'$File' is not an installer name a trusted comment carries" }
    $b64 = {
        param($line, $length)
        if ($line -cnotmatch '\A[A-Za-z0-9+/]+={0,2}\z') { return $null }
        $bytes = [Convert]::FromBase64String($line)
        if ($bytes.Length -ne $length -or [Convert]::ToBase64String($bytes) -cne $line) { return $null }
        , $bytes
    }
    if ($Signature -cnotmatch '\A[A-Za-z0-9+/]+={0,2}\z') { throw 'the signature is not base64 with nothing around it' }
    $raw = [Convert]::FromBase64String($Signature)
    if ([Convert]::ToBase64String($raw) -cne $Signature) { throw 'the signature is not canonical base64' }
    if (@($raw | Where-Object { $_ -gt 0x7E -or ($_ -lt 0x20 -and $_ -ne 0x09 -and $_ -ne 0x0A) }).Count) {
        throw 'the signature file holds what is not printable ASCII'
    }
    $text = [Text.Encoding]::ASCII.GetString($raw)
    if (-not $text.EndsWith("`n")) { throw 'the signature file does not end its last line' }
    $lines = $text.Substring(0, $text.Length - 1) -split "`n"
    if ($lines.Count -ne 4) { throw 'the signature file does not have the 4 lines of a minisign signature' }
    if ($lines[0] -cne "untrusted comment: $UpdaterUntrustedComment") { throw 'the signature''s untrusted comment is not Tauri''s' }
    $bin = & $b64 $lines[1] 74
    if (-not $bin -or $bin[0] -ne 0x45 -or $bin[1] -ne 0x44) { throw 'not a prehashed Ed25519 minisign signature' }
    if (-not (Test-SameBytes ([byte[]]$bin[2..9]) (Read-UpdaterPubkey $Pubkey).KeyId)) { throw 'signed under another key id than the updater''s' }
    $comment = "trusted comment: timestamp:(?<at>[0-9]{1,10})`tfile:$([regex]::Escape($File))`tversion:$([regex]::Escape($Version))`tcore_fingerprint:$Fingerprint"
    $match = [regex]::Match($lines[2], "\A$comment\z", [Text.RegularExpressions.RegexOptions]::CultureInvariant)
    if (-not $match.Success) { throw "the trusted comment is not the one for $File, $Version and core $Fingerprint" }
    $at = [long]$match.Groups['at'].Value
    if ($null -ne $Timestamp) {
        if ($at -ne $Timestamp) { throw 'the signature is not dated as it was signed' }
    } elseif ($at -gt [DateTimeOffset]::UtcNow.AddDays(1).ToUnixTimeSeconds()) {
        throw 'the signature is dated ahead of this clock'
    }
    if (-not (& $b64 $lines[3] 64)) { throw 'the global signature is not 64 bytes of canonical base64' }
}

# Checks $Signature (a .sig's text, in the form Assert-UpdaterSignatureForm
# holds it to) over the file at $Path under the public key $Pubkey with
# OpenSSL: the Ed25519 signature over the file's BLAKE2b-512, and the global
# signature over that signature and the trusted comment, as minisign defines
# them and installed copies check them. The same check with one byte of the
# hash changed must fail too, so a tool that passes everything passes
# nothing. $OpenSsl: Get-OpenSsl's when not given. Work files go to a folder
# of its own under $Root's target.
function Assert-UpdaterSignatureVerifies([string]$Path, [string]$Signature, [string]$Pubkey, [string]$Root = $RepoRoot, [string]$OpenSsl) {
    if (-not $OpenSsl) { $OpenSsl = Get-OpenSsl }
    $lines = [Text.Encoding]::ASCII.GetString([Convert]::FromBase64String($Signature)).TrimEnd("`n") -split "`n"
    $bin = [Convert]::FromBase64String($lines[1])
    $comment = [Text.Encoding]::ASCII.GetBytes($lines[2].Substring('trusted comment: '.Length))
    $key = (Read-UpdaterPubkey $Pubkey).Public
    $work = New-SigningWork $Root
    try {
        $file = { param($name, [byte[]]$bytes) $at = Join-Path $work $name; [IO.File]::WriteAllBytes($at, $bytes); $at }
        $pub = & $file 'pub.der' ([byte[]]($Ed25519SpkiPrefix + $key))
        $sig = & $file 'sig.bin' ([byte[]]$bin[10..73])
        $global = & $file 'global.bin' ([Convert]::FromBase64String($lines[3]))
        $signed = & $file 'signed.bin' ([byte[]]([byte[]]$bin[10..73] + $comment))
        $hash = Join-Path $work 'hash.bin'
        [IO.File]::WriteAllBytes($hash, (Invoke-OpenSsl $OpenSsl "hashing $Path" @('dgst', '-blake2b512', '-binary', $Path)))
        if ((Get-Item -LiteralPath $hash).Length -ne 64) { throw "OpenSSL could not hash $Path" }
        $verifies = {
            param($message, $signature)
            $said = try {
                $arguments = @('pkeyutl', '-verify', '-pubin', '-inkey', $pub, '-keyform', 'DER', '-rawin', '-in', $message, '-sigfile', $signature)
                [Text.Encoding]::ASCII.GetString((Invoke-OpenSsl $OpenSsl 'verifying' $arguments))
            } catch { '' }
            $said.Trim() -ceq 'Signature Verified Successfully'
        }
        if (-not (& $verifies $hash $sig)) { throw 'the signature does not verify over the installer under the updater''s key (OpenSSL)' }
        if (-not (& $verifies $signed $global)) { throw 'the trusted comment is not the one signed (OpenSSL)' }
        $other = [IO.File]::ReadAllBytes($hash)
        $other[0] = $other[0] -bxor 1
        if (& $verifies (& $file 'other.bin' $other) $sig) { throw "OpenSSL at $OpenSsl takes a signature over other bytes; it cannot check this one" }
    } finally {
        Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
    }
}

# A new folder for a signing's work files under $Root's target folder, which
# git ignores: the key encrypted for OpenSSL, the messages it signs, what a
# check writes. Removed by its caller.
function New-SigningWork([string]$Root = $RepoRoot) {
    $work = Join-Path $Root "src-tauri/target/release-signing/$([guid]::NewGuid().ToString('n'))"
    New-Item -ItemType Directory -Force $work | Out-Null
    $work
}

# Checks the updater signature of the release whose assets are in $Dir as
# installed copies will check it, for a draft (docs/releasing.md, "Verifying
# a release"), with OpenSSL and no build: the installer for $Version, its
# .sig in the signing's form (Assert-UpdaterSignatureForm) and verifying
# (Assert-UpdaterSignatureVerifies) under $Pubkey (updater.json's when not
# given; for a bridge release, the old key) for $Version and the core
# latest.json names, and latest.json carrying that same signature.
function Assert-ReleaseSignature([string]$Dir, [string]$Version, [string]$Pubkey, [string]$Root = $RepoRoot) {
    Assert-VersionFormat $Version
    $Dir = [IO.Path]::GetFullPath($Dir, (Get-Location).Path)
    $setups = @(Get-ChildItem -LiteralPath $Dir -Filter "*_${Version}_*-setup.exe" -File -ErrorAction SilentlyContinue)
    if ($setups.Count -ne 1) { throw "$Dir should hold one installer for $Version, not $($setups.Count)" }
    $setup = $setups[0]
    $signature = Get-Content -LiteralPath "$($setup.FullName).sig" -Raw
    $manifest = Get-Content -LiteralPath (Join-Path $Dir 'latest.json') -Raw | ConvertFrom-Json -AsHashtable
    if ($manifest.version -cne $Version) { throw "latest.json is for version '$($manifest.version)', not $Version" }
    if ($manifest.platforms.'windows-x86_64'.signature -cne $signature) { throw "latest.json carries another signature than $($setup.Name).sig" }
    Assert-CoreFingerprint $manifest.core_fingerprint 'latest.json'
    if (-not $Pubkey) { $Pubkey = Get-UpdaterPubkey $Root }
    Assert-UpdaterSignatureForm $signature $Pubkey $setup.Name $Version $manifest.core_fingerprint
    Assert-UpdaterSignatureVerifies $setup.FullName $signature $Pubkey $Root
    Write-Host "$($setup.Name).sig verifies under the updater's key for $Version, core $($manifest.core_fingerprint) (OpenSSL)"
}

# Each file of the folder $Dir, by name, with its SHA-256: what a bundle is
# held to across its signing. A folder in it, or a link, is a throw.
function Get-BundleState([string]$Dir) {
    $state = [ordered]@{}
    foreach ($item in Get-ChildItem -LiteralPath $Dir -Force | Sort-Object Name) {
        if ($item.PSIsContainer -or (Test-Link $item.FullName)) { throw "$($item.Name) in $Dir is not a plain file" }
        $state[$item.Name] = (Get-FileHash -LiteralPath $item.FullName -Algorithm SHA256).Hash
    }
    $state
}

# Signs the installer for $Version in the bundle $Dir (release-bundle.ps1's)
# and puts the signature into its latest.json; release-sign.ps1 says why and
# how. In order: the key is opened (Read-UpdaterSecretKey) and held to the
# public key the signature is checked against, $TrustedPubkey or else the
# one $Root's updater.json holds, so a key installed copies would refuse
# stops here before anything is signed; the installer is signed, the trusted
# comment naming it, $Version and the core fingerprint latest.json carries,
# dated as it is signed (New-UpdaterSignature); the .sig is held to its form
# (Assert-UpdaterSignatureForm) and OpenSSL verifies both its signatures
# under that public key (Assert-UpdaterSignatureVerifies); the bundle must be
# file for file what it was before; only then does the .sig go into the
# bundle and latest.json take it.
#
# The key: with -Throwaway, one made for the run and kept in memory
# (New-UpdaterKeyPair), what a dry run signs with; else the file $UpdaterKey
# or TAURI_SIGNING_PRIVATE_KEY, the release environment's secret, opened with
# TAURI_SIGNING_PRIVATE_KEY_PASSWORD, empty when unset (the maintainer's key
# has none). $TrustedPubkey is for a key rotation's bridge release, which
# embeds the new public key but is signed with the old private key, the one
# installed copies still trust (docs/releasing.md): the old public key, given
# by whoever releases, never taken from the build. $Now is the clock the
# trusted comment is dated by, here for the tests.
function Invoke-ReleaseSign {
    param(
        [Parameter(Mandatory)][string]$Dir,
        [Parameter(Mandatory)][string]$Version,
        [string]$UpdaterKey,
        [string]$TrustedPubkey,
        # A key made for this run alone signs and is checked against: what a
        # dry run does to go through the signing without the release's key.
        # Nothing it signs is ever published.
        [switch]$Throwaway,
        [string]$Root = $RepoRoot,
        [scriptblock]$Now = { [DateTimeOffset]::UtcNow }
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
    # The core the build reported (release-bundle.ps1, from build.rs's file),
    # signed into the trusted comment: 0.20's updater takes it from there.
    # Set-UpdaterManifestSignature checks the rest of the manifest below.
    $fingerprint = (Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json -AsHashtable).core_fingerprint
    Assert-CoreFingerprint $fingerprint 'latest.json'
    $embedded = Get-UpdaterPubkey $Root
    $pubkey = $embedded
    if (-not $Throwaway -and $TrustedPubkey -and $TrustedPubkey.Trim() -cne $embedded) {
        $pubkey = $TrustedPubkey.Trim()
        Write-Host 'checking the signature against the trusted public key given for a rotation, not the one this release embeds'
    }
    $endpoint = Get-UpdaterEndpoint $Root
    $openssl = Get-OpenSsl -ForKey
    $which = [Text.Encoding]::ASCII.GetString((Invoke-OpenSsl $openssl 'asking its version' @('version'))).Trim()
    Write-Host "signing with $which ($openssl)"

    $password = ''
    if ($Throwaway) {
        $pair = New-UpdaterKeyPair $openssl
        $keyText = $pair.Secret
        $pubkey = $pair.Public
        Write-Host 'signing with a throwaway key made for this run; its signature is checked against that key''s public half'
    } elseif ($UpdaterKey) {
        if (-not (Test-Path -LiteralPath $UpdaterKey -PathType Leaf)) { throw "no updater key at $UpdaterKey" }
        $keyText = [IO.File]::ReadAllText((Resolve-Path -LiteralPath $UpdaterKey).Path)
        $password = "$env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD"
    } elseif ($env:TAURI_SIGNING_PRIVATE_KEY) {
        $keyText = $env:TAURI_SIGNING_PRIVATE_KEY
        $password = "$env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD"
    } else {
        throw 'signing needs the updater''s private key: pass -UpdaterKey <path>, or set LUNA_UPDATER_KEY or TAURI_SIGNING_PRIVATE_KEY'
    }
    $trusted = Read-UpdaterPubkey $pubkey

    $before = Get-BundleState $Dir
    $work = New-SigningWork $Root
    $key = $null
    try {
        $key = Read-UpdaterSecretKey $openssl $keyText $password
        if (-not (Test-SameBytes $key.KeyId $trusted.KeyId) -or -not (Test-SameBytes $key.Public $trusted.Public)) {
            throw 'the updater key given is not the one the signature is checked against (updater.json''s public key, or the trusted one given for a rotation): installed copies would refuse what it signs'
        }
        $signed = New-UpdaterSignature -OpenSsl $openssl -Key $key -Path $setup.FullName -Version $Version -Fingerprint $fingerprint -Work $work -Now $Now
        $signature = $signed.Signature
        Assert-UpdaterSignatureForm $signature $pubkey $setup.Name $Version $fingerprint -Timestamp $signed.Timestamp
        Assert-UpdaterSignatureVerifies $setup.FullName $signature $pubkey $Root -OpenSsl $openssl
        $after = Get-BundleState $Dir
        if ((($before.GetEnumerator() | ForEach-Object { "$($_.Key) $($_.Value)" }) -join "`n") -cne (($after.GetEnumerator() | ForEach-Object { "$($_.Key) $($_.Value)" }) -join "`n")) {
            throw "$Dir changed while the installer was signed"
        }
        $manifest = Set-UpdaterManifestSignature -Manifest (Get-Content -LiteralPath $manifestPath -Raw) -Signature $signature `
            -Version $Version -SetupName $setup.Name -Endpoint $endpoint
        [IO.File]::WriteAllText($sigPath, $signature, [Text.UTF8Encoding]::new($false))
        $manifest | Set-Content -LiteralPath $manifestPath -Encoding utf8
        Write-Host "signed $($setup.Name) for v$Version, core $fingerprint; OpenSSL verified it under the updater's key; latest.json carries it"
    } finally {
        if ($key) { [Array]::Clear($key.Seed) }
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

# The updater endpoint every installed Luna polls, which latest.json is
# published for (src-tauri/updater.json).
function Get-UpdaterEndpoint([string]$Root = $RepoRoot) {
    (Read-UpdaterSource $Root).Endpoint
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
