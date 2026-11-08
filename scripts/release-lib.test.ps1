#Requires -Version 7.4
# Tests for release-lib.ps1, on copies of the repo's own version files in a
# folder of their own under test-build/.
# Run: pwsh scripts/release-lib.test.ps1 (the gate runs it with the frontend group)
. "$PSScriptRoot/lib.ps1"
. "$PSScriptRoot/release-lib.ps1"

$failures = 0
function Check([string]$Name, [scriptblock]$Test) {
    try {
        & $Test
        Write-Host "ok   $Name"
    } catch {
        $script:failures++
        Write-Host "FAIL ${Name}: $_" -ForegroundColor Red
    }
}
function Assert-Equal($Actual, $Expected) {
    if ($Actual -cne $Expected) { throw "expected '$Expected', got '$Actual'" }
}
function Assert-Throws([scriptblock]$Block, [string]$Like) {
    try { & $Block } catch {
        if ("$_" -notlike "*$Like*") { throw "threw '$_', not *$Like*" }
        return
    }
    throw 'did not throw'
}

$work = Join-Path $RepoRoot "test-build/release-lib-tests/$([guid]::NewGuid().ToString('n'))"

# A fresh tree holding copies of the version files, and of tauri.conf.json's
# updater endpoint with them.
function New-Tree([string]$Name) {
    $root = Join-Path $work $Name
    foreach ($file in $ReleaseVersionFiles.Keys) {
        $to = Join-Path $root $file
        New-Item -ItemType Directory -Force (Split-Path $to) | Out-Null
        Copy-Item -LiteralPath (Join-Path $RepoRoot $file) -Destination $to
    }
    $root
}

# The lines of two texts that differ, in order; both split on LF only, so a
# changed line ending shows as a changed line.
function Compare-Lines([string]$Before, [string]$After) {
    $a = $Before -split "`n"; $b = $After -split "`n"
    if ($a.Count -ne $b.Count) { throw "line count changed: $($a.Count) -> $($b.Count)" }
    @(for ($i = 0; $i -lt $a.Count; $i++) { if ($a[$i] -cne $b[$i]) { $b[$i] } })
}

try {
    $current = (Get-Content (Join-Path $RepoRoot 'package.json') -Raw | ConvertFrom-Json).version

    Check 'the repo''s four version files agree' {
        Assert-Equal (Get-ReleaseVersion) $current
    }

    Check 'a bump changes the version line of each file and nothing else' {
        $root = New-Tree bump
        $before = @{}
        foreach ($file in $ReleaseVersionFiles.Keys) { $before[$file] = [IO.File]::ReadAllText((Join-Path $root $file)) }
        Set-ReleaseVersion '12.34.56-rc.1' $root
        Assert-Equal (Get-ReleaseVersion $root) '12.34.56-rc.1'
        foreach ($file in $ReleaseVersionFiles.Keys) {
            $changed = @(Compare-Lines $before[$file] ([IO.File]::ReadAllText((Join-Path $root $file))))
            if ($changed.Count -ne 1 -or $changed[0] -notmatch '12\.34\.56-rc\.1') { throw "$file changed in: $($changed -join ' | ')" }
        }
    }

    Check 'a bump keeps CRLF line endings' {
        $root = New-Tree crlf
        foreach ($file in 'package.json', 'src-tauri/Cargo.toml', 'src-tauri/Cargo.lock') {
            $path = Join-Path $root $file
            [IO.File]::WriteAllText($path, ([IO.File]::ReadAllText($path) -replace "`r?`n", "`r`n"))
        }
        Set-ReleaseVersion '2.0.0' $root
        Assert-Equal (Get-ReleaseVersion $root) '2.0.0'
        foreach ($file in 'package.json', 'src-tauri/Cargo.toml', 'src-tauri/Cargo.lock') {
            $text = [IO.File]::ReadAllText((Join-Path $root $file))
            if ($text -match "[^`r]`n") { throw "$file has a bare LF after the bump" }
        }
    }

    Check 'versions that disagree are refused, each file named' {
        $root = New-Tree disagree
        $path = Join-Path $root 'src-tauri/Cargo.toml'
        $text = [IO.File]::ReadAllText($path)
        [IO.File]::WriteAllText($path, $text.Replace("version = `"$current`"", 'version = "0.0.1"'))
        Assert-Throws { Get-ReleaseVersion $root } "src-tauri/Cargo.toml 0.0.1"
    }

    Check 'only versions of the form x.y.z(-pre) are written' {
        $root = New-Tree format
        Assert-Throws { Set-ReleaseVersion 'v1.2.3' $root } 'not a version'
        Assert-Throws { Set-ReleaseVersion '1.2' $root } 'not a version'
        Assert-Equal (Get-ReleaseVersion $root) $current
    }

    Check 'a v* tag is a signed release, published unless a dry run' {
        $plan = Get-ReleasePlan 'refs/tags/v1.2.3' '1.2.3' $false
        Assert-Equal "$($plan.Tag) $($plan.Signed) $($plan.Publish)" 'v1.2.3 True True'
        $plan = Get-ReleasePlan 'refs/tags/v1.2.3' '1.2.3' $true
        Assert-Equal "$($plan.Signed) $($plan.Publish)" 'True False'
        Assert-Throws { Get-ReleasePlan 'refs/tags/v1.2.4' '1.2.3' $false } 'does not name the version'
        Assert-Throws { Get-ReleasePlan 'refs/tags/V1.2.3' '1.2.3' $true } 'does not name the version'
    }

    Check 'a branch is only ever an unsigned dry run' {
        $plan = Get-ReleasePlan 'refs/heads/master' '1.2.3' $true
        Assert-Equal "$($plan.Tag) $($plan.Signed) $($plan.Publish)" 'v1.2.3 False False'
        Assert-Throws { Get-ReleasePlan 'refs/heads/master' '1.2.3' $false } 'from its v* tag'
    }

    Check 'CI is green once a run of the commit has succeeded' {
        $run = { param($s, $c) [pscustomobject]@{ status = $s; conclusion = $c } }
        Assert-Equal (Get-CiVerdict @()) 'missing'
        Assert-Equal (Get-CiVerdict $null) 'missing'
        Assert-Equal (Get-CiVerdict @((& $run 'in_progress' $null))) 'pending'
        Assert-Equal (Get-CiVerdict @((& $run 'completed' 'failure'), (& $run 'queued' $null))) 'pending'
        Assert-Equal (Get-CiVerdict @((& $run 'completed' 'failure'), (& $run 'completed' 'timed_out'))) 'red'
        Assert-Equal (Get-CiVerdict @((& $run 'completed' 'failure'), (& $run 'completed' 'success'))) 'green'
    }

    Check 'versions compare as semver, a release above its pre-releases' {
        Assert-Equal (Compare-ReleaseVersion '0.10.0' '0.9.10') 1
        Assert-Equal (Compare-ReleaseVersion '0.9.10' '0.10.0') -1
        Assert-Equal (Compare-ReleaseVersion '1.2.3' '1.2.3') 0
        Assert-Equal (Compare-ReleaseVersion '1.2.3' '1.2.3-rc.1') 1
        Assert-Equal (Compare-ReleaseVersion '1.2.3-rc.1' '1.2.3-rc.2') -1
    }

    # A CHANGELOG as the repo writes it: the Unreleased section, three versions
    # with subsections, a bare one, an empty one, and the link definitions.
    $changelog = @'
# Changelog

All notable changes are recorded here.

## [Unreleased]

### Fixed

- Not out yet.

## [0.10.1] - 2026-11-09

A small release.

### Fixed

- The release workflow's SBOM attestation.

## [0.10.0] - 2026-11-08

A security and engineering release. Agent tools now follow least privilege.

### Security

- Helpers hold no more privilege than their starter.

### Added

- A threat model (`SECURITY.md`).

## [0.9.11] - 2026-11-07

## [0.9.10] - 2026-11-06

- Fixes and refinements.

[Unreleased]: https://github.com/nrubinmosin/luna/compare/v0.10.1...HEAD
[0.10.1]: https://github.com/nrubinmosin/luna/compare/v0.10.0...v0.10.1
'@ -replace "`r`n", "`n"
    $tree = New-Tree notes
    Set-Content (Join-Path $tree 'CHANGELOG.md') $changelog -NoNewline

    Check 'a CHANGELOG section is extracted exactly, intro and subsections' {
        $section = Get-ChangelogSection '0.10.0' $tree
        Assert-Equal $section.Date '2026-11-08'
        Assert-Equal $section.Heading '[0.10.0] - 2026-11-08'
        Assert-Equal $section.Intro 'A security and engineering release. Agent tools now follow least privilege.'
        $expected = @'
A security and engineering release. Agent tools now follow least privilege.

### Security

- Helpers hold no more privilege than their starter.

### Added

- A threat model (`SECURITY.md`).
'@ -replace "`r`n", "`n"
        Assert-Equal $section.Body $expected.Trim()
        # The last section ends before the link definitions, and may have no intro.
        $last = Get-ChangelogSection '0.9.10' $tree
        Assert-Equal $last.Body '- Fixes and refinements.'
        Assert-Equal $last.Intro '- Fixes and refinements.'
        Assert-Equal ((Get-ChangelogVersions $tree) -join ' ') '0.10.1 0.10.0 0.9.11 0.9.10'
    }

    Check 'a missing or empty CHANGELOG section is refused, and Unreleased never taken' {
        Assert-Throws { Get-ChangelogSection '0.9.9' $tree } 'no section for 0.9.9'
        Assert-Throws { Get-ChangelogSection '0.9.11' $tree } '0.9.11 section of CHANGELOG.md is empty'
        Assert-Throws { Get-ChangelogSection 'Unreleased' $tree } 'not a version'
        $bare = Join-Path $work 'no-changelog'
        New-Item -ItemType Directory -Force $bare | Out-Null
        Assert-Throws { Get-ChangelogSection '0.10.0' $bare } 'no CHANGELOG.md'
        Assert-Throws { New-ReleaseNotes -Version '0.9.11' -Previous 'v0.9.10' -Root $tree } 'is empty'
        # An empty section among those the notes cover is refused too: 0.9.11
        # lies between 0.10.1 and the published 0.9.10.
        Assert-Throws { New-ReleaseNotes -Version '0.10.1' -Previous 'v0.9.10' -Root $tree } '0.9.11 section of CHANGELOG.md is empty'
    }

    Check 'release notes span every CHANGELOG version since the last published release' {
        # 0.10.0 was tagged but never published: 0.10.1's notes carry it.
        $notes = New-ReleaseNotes -Version '0.10.1' -Previous 'v0.9.11' -Root $tree
        $expected = @'
A small release.

### Fixed

- The release workflow's SBOM attestation.

## Also from 0.10.0

0.10.0 had no published release of its own; its changes first ship here.

A security and engineering release. Agent tools now follow least privilege.

### Security

- Helpers hold no more privilege than their starter.

### Added

- A threat model (`SECURITY.md`).

Full changes: [v0.9.11...v0.10.1](https://github.com/nrubinmosin/luna/compare/v0.9.11...v0.10.1) and the [CHANGELOG](https://github.com/nrubinmosin/luna/blob/v0.10.1/CHANGELOG.md#0101---2026-11-09).

'@ -replace "`r`n", "`n"
        Assert-Equal $notes $expected
        # With 0.10.0 published, 0.10.1's notes stop at its own section.
        $notes = New-ReleaseNotes -Version '0.10.1' -Previous 'v0.10.0' -Root $tree
        if ($notes -match 'Also from') { throw "0.10.0's section was included: $notes" }
        if ($notes -notmatch '\[v0\.10\.0\.\.\.v0\.10\.1\]') { throw "no compare link from v0.10.0: $notes" }
        # Nothing published yet: the version's section alone, no compare link.
        $notes = New-ReleaseNotes -Version '0.10.1' -Previous '' -Root $tree
        if ($notes -match 'Also from|compare/') { throw "an unpublished repo got a span or compare link: $notes" }
        if ($notes -notmatch 'Full changes: the \[CHANGELOG\]') { throw "no CHANGELOG link: $notes" }
        # Unreleased is not a version: a span never reaches it, and it cannot be released.
        if ($notes -match 'Not out yet') { throw 'the Unreleased section leaked into the notes' }
        Assert-Throws { New-ReleaseNotes -Version 'Unreleased' -Previous '' -Root $tree } 'not a version'
    }

    Check 'release notes end on the verify commands and the checksums' {
        $sums = @'
ff84  Luna_0.10.1_x64-setup.exe
ddb7  Luna_0.10.1_x64-setup.exe.sig
d8e1  latest.json
'@ -replace "`r`n", "`n"
        $notes = New-ReleaseNotes -Version '0.10.1' -Previous 'v0.10.0' -Checksums $sums -Root $tree
        $expected = @'
## Verify

```sh
# Built by this repository's release workflow, from the tagged commit.
gh attestation verify Luna_0.10.1_x64-setup.exe --repo nrubinmosin/luna \
  --signer-workflow nrubinmosin/luna/.github/workflows/release.yml

# The SBOMs attested against the installer.
gh attestation verify Luna_0.10.1_x64-setup.exe --repo nrubinmosin/luna \
  --predicate-type https://cyclonedx.org/bom

# Every asset against the checksums.
sha256sum -c SHA256SUMS
```

SHA256SUMS:

```text
ff84  Luna_0.10.1_x64-setup.exe
ddb7  Luna_0.10.1_x64-setup.exe.sig
d8e1  latest.json
```

'@ -replace "`r`n", "`n"
        if (-not $notes.EndsWith($expected)) { throw "the notes do not end on the verify block:`n$notes" }
        Assert-Throws { New-ReleaseNotes -Version '0.10.1' -Previous 'v0.10.0' -Checksums "d8e1  latest.json`n" -Root $tree } 'no *-setup.exe'
        $without = New-ReleaseNotes -Version '0.10.1' -Previous 'v0.10.0' -Root $tree
        if ($without -match 'Verify|SHA256SUMS') { throw 'a verify block without checksums' }
    }

    # Byte for byte what scripts/release.ps1 wrote before the workflow, for the
    # same inputs: the updater on every installed Luna reads this shape.
    Check 'latest.json is written as it always was' {
        $json = New-UpdaterManifest -Version '0.9.10' -CoreFingerprint '0123456789abcdef' -Signature 'c2lnbmF0dXJl' `
            -SetupName 'Luna_0.9.10_x64-setup.exe' -Endpoint (Get-UpdaterEndpoint) `
            -PubDate ([datetime]::new(2026, 11, 7, 12, 30, 5, [DateTimeKind]::Utc))
        $expected = @'
{
  "version": "0.9.10",
  "core_fingerprint": "0123456789abcdef",
  "notes": "Luna 0.9.10",
  "pub_date": "2026-11-07T12:30:05Z",
  "platforms": {
    "windows-x86_64": {
      "signature": "c2lnbmF0dXJl",
      "url": "https://github.com/nrubinmosin/luna/releases/download/v0.9.10/Luna_0.9.10_x64-setup.exe"
    }
  }
}
'@ -replace "`r`n", "`n"
        Assert-Equal ($json -replace "`r`n", "`n") $expected
        $notes = New-UpdaterManifest -Version '1.0.0' -CoreFingerprint '0123456789abcdef' -Signature 's' -SetupName 'x' `
            -Endpoint (Get-UpdaterEndpoint) -Notes "Line one`nLine two" | ConvertFrom-Json
        Assert-Equal $notes.notes "Line one`nLine two"
        Assert-Throws {
            New-UpdaterManifest -Version '1.0.0' -CoreFingerprint 'f' -Signature 's' -SetupName 'x' -Endpoint 'https://example.com/latest.json'
        } 'not a GitHub latest-release'
    }

    Check 'the release build''s artifacts are found by version' {
        $dir = Join-Path $work 'build'
        $nsis = New-Item -ItemType Directory -Force (Join-Path $dir 'bundle/nsis')
        foreach ($name in 'Luna_0.9.1_x64-setup.exe', 'Luna_0.9.1_x64-setup.exe.sig', 'Luna_0.9.10_x64-setup.exe') {
            Set-Content (Join-Path $nsis $name) 'x'
        }
        Set-Content (Join-Path $dir 'luna.exe') 'x'
        Assert-Throws { Find-ReleaseArtifacts '0.9.10' -Unsigned -Dir $dir } 'core-fingerprint.txt'
        Set-Content (Join-Path $dir 'core-fingerprint.txt') '0123456789abcdef' -NoNewline
        Assert-Throws { Find-ReleaseArtifacts '0.9.10' -Dir $dir } 'Luna_0.9.10_x64-setup.exe.sig'
        $found = Find-ReleaseArtifacts '0.9.10' -Unsigned -Dir $dir
        Assert-Equal (Split-Path -Leaf $found.Setup) 'Luna_0.9.10_x64-setup.exe'
        Assert-Equal $found.Signature $null
        $found = Find-ReleaseArtifacts '0.9.1' -Dir $dir
        Assert-Equal (Split-Path -Leaf $found.Signature) 'Luna_0.9.1_x64-setup.exe.sig'
        Assert-Throws { Find-ReleaseArtifacts '0.9.2' -Dir $dir } 'no installer for 0.9.2'
    }

    Check 'an SBOM is checked the way actions/attest checks it' {
        $dir = New-Item -ItemType Directory -Force (Join-Path $work 'sbom')
        $sbom = { param($name, $json) $path = Join-Path $dir $name; Set-Content $path $json; $path }
        Assert-AttestableSbom (& $sbom 'cargo.cdx.json' '{"bomFormat":"CycloneDX","specVersion":"1.5","version":1,"serialNumber":"urn:uuid:755d855a-6e1c-4bd8-84fe-56baa8b7d251","metadata":{}}')
        # What cyclonedx-npm --output-reproducible wrote for v0.10.0: no serial number.
        Assert-Throws {
            Assert-AttestableSbom (& $sbom 'npm.cdx.json' '{"$schema":"http://cyclonedx.org/schema/bom-1.5.schema.json","bomFormat":"CycloneDX","specVersion":"1.5","version":1}')
        } 'npm.cdx.json has no serialNumber'
        Assert-Throws {
            Assert-AttestableSbom (& $sbom 'empty.cdx.json' '{"bomFormat":"CycloneDX","specVersion":"","serialNumber":""}')
        } 'no serialNumber, specVersion'
        Assert-Throws {
            Assert-AttestableSbom (& $sbom 'spdx.json' '{"spdxVersion":"SPDX-2.3","SPDXID":"SPDXRef-DOCUMENT"}')
        } 'not CycloneDX'
        Assert-Throws { Assert-AttestableSbom (& $sbom 'list.json' '[]') } 'not a JSON object'
    }

    Check 'a core fingerprint is 16 lowercase hex digits' {
        Assert-CoreFingerprint '0123456789abcdef' 'test'
        Assert-Throws { Assert-CoreFingerprint '0123456789ABCDEF' 'x.txt' } 'x.txt holds'
        Assert-Throws { Assert-CoreFingerprint '' 'x.txt' } 'x.txt holds'
    }
} finally {
    Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
}

if ($failures) {
    Write-Host "$failures release-lib test(s) failed" -ForegroundColor Red
    exit 1
}
Write-Host 'release-lib tests passed' -ForegroundColor Green
