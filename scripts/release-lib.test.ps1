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

# A workflow's jobs, read by indentation as YAML nests them (no YAML parser
# ships with pwsh or the toolchain images): each job, two spaces in under
# `jobs:`, with Lines, its own lines, comments dropped, and Steps, each item
# of its `steps:` list with Keys, every key of the step and its whole value:
# the text after the colon and every line indented deeper than the key, which
# takes in block scalars (`run: |`, `>-`), quoted and plain multi-line values
# and nested maps alike. Header holds the lines before `jobs:`.
function Read-WorkflowJobs([string]$Text) {
    $lines = @($Text -split '\r?\n' | Where-Object { $_ -notmatch '^\s*#' })
    $jobsAt = [array]::IndexOf($lines, ($lines | Where-Object { $_ -match '^jobs:\s*$' } | Select-Object -First 1))
    if ($jobsAt -lt 0) { throw 'no jobs:' }
    $indent = { param($line) $line.Length - $line.TrimStart(' ').Length }
    $jobs = [ordered]@{}
    $job = $null
    for ($i = $jobsAt + 1; $i -lt $lines.Count; $i++) {
        $line = $lines[$i]
        if ($line -match '^  ([A-Za-z0-9_-]+):\s*$') {
            $job = [pscustomobject]@{ Lines = [System.Collections.Generic.List[string]]::new(); Steps = [System.Collections.Generic.List[object]]::new() }
            $jobs[$Matches[1]] = $job
            continue
        }
        if ($line.Trim() -and (& $indent $line) -lt 2) { throw "line $($i + 1) is outside any job: $line" }
        if ($job) { $job.Lines.Add($line) }
    }
    foreach ($job in $jobs.Values) {
        $body = $job.Lines
        $stepsAt = -1
        for ($i = 0; $i -lt $body.Count; $i++) { if ($body[$i] -match '^    steps:\s*$') { $stepsAt = $i; break } }
        if ($stepsAt -lt 0) { continue }
        # Each item: its lines, its "- " opening taken as two more spaces of
        # indentation, so its first key sits with the others.
        $items = [System.Collections.Generic.List[object]]::new()
        for ($i = $stepsAt + 1; $i -lt $body.Count; $i++) {
            $line = $body[$i]
            if ($line.Trim() -and (& $indent $line) -le 4) { break }
            if ($line -match '^(\s*)- (.*)$' -and $Matches[1].Length -eq 6) {
                $items.Add([System.Collections.Generic.List[string]]::new())
                $items[-1].Add((' ' * 8) + $Matches[2])
            } elseif ($items.Count) {
                $items[-1].Add($line)
            } elseif ($line.Trim()) {
                throw "a step line before any step: $line"
            }
        }
        foreach ($item in $items) {
            $keys = [ordered]@{}
            for ($i = 0; $i -lt $item.Count; $i++) {
                if ($item[$i] -notmatch '^        ([A-Za-z0-9_-]+):(.*)$') {
                    if ($item[$i].Trim() -and (& $indent $item[$i]) -le 8) { throw "not a step key: $($item[$i])" }
                    continue
                }
                $name = $Matches[1]
                $value = [System.Collections.Generic.List[string]]::new()
                $value.Add($Matches[2].Trim())
                while ($i + 1 -lt $item.Count -and (-not $item[$i + 1].Trim() -or (& $indent $item[$i + 1]) -gt 8)) {
                    $i++
                    $value.Add($item[$i].Trim())
                }
                $keys[$name] = ($value -join "`n").Trim()
            }
            $job.Steps.Add([pscustomobject]@{ Keys = $keys })
        }
    }
    [pscustomobject]@{ Header = @($lines[0..([math]::Max($jobsAt - 1, 0))]); Jobs = $jobs }
}

# The sign job as release.yml has to have it, step for step: every key of
# every step and its whole value as Read-WorkflowJobs gives it (a `uses:`
# without its version comment). An allowlist, not a list of what is barred:
# a step, a key or a value that is not here is refused, however its command
# is spelled. A change to the sign job changes this list in the same commit.
$SignJob = @'
    needs: [verify, build]
    runs-on: ubuntu-24.04
    timeout-minutes: 15
    environment: ${{ needs.verify.outputs.signed == 'true' && 'release' || '' }}
    permissions:
      contents: read
    steps:
'@
$SignSteps = @(
    [ordered]@{
        uses = 'actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1'
        with = @'
persist-credentials: false
sparse-checkout-cone-mode: false
sparse-checkout: |
/scripts/lib.ps1
/scripts/release-lib.ps1
/scripts/release-sign.ps1
/scripts/updater-signature.mjs
/docker/windows-build.Dockerfile
/pnpm-lock.yaml
/src-tauri/tauri.conf.json
'@
    }
    [ordered]@{
        uses = 'actions/download-artifact@9000827ccba6bdab643e8b6fd33ac0654aef8333'
        with = @'
name: unsigned
path: ${{ runner.temp }}/assets
'@
    }
    [ordered]@{
        name = 'Check the bundle against the build''s checksums'
        env  = 'SUMS: ${{ needs.build.outputs.sums }}'
        run  = @'
|
. ./scripts/lib.ps1
. ./scripts/release-lib.ps1
Assert-AssetChecksums "$env:RUNNER_TEMP/assets" $env:SUMS
'@
    }
    [ordered]@{
        name = 'Sign the installer'
        if   = 'needs.verify.outputs.signed == ''true'''
        env  = @'
VERSION: ${{ needs.verify.outputs.version }}
LUNA_UPDATER_TRUSTED_PUBKEY: ${{ vars.UPDATER_TRUSTED_PUBKEY }}
TAURI_SIGNING_PRIVATE_KEY: ${{ secrets.TAURI_SIGNING_PRIVATE_KEY }}
TAURI_SIGNING_PRIVATE_KEY_PASSWORD: ${{ secrets.TAURI_SIGNING_PRIVATE_KEY_PASSWORD }}
'@
        run  = './scripts/release-sign.ps1 -Dir "$env:RUNNER_TEMP/assets" -Version $env:VERSION -RequireTrustedPubkey'
    }
    [ordered]@{
        uses = 'actions/upload-artifact@cf430e030ddbb5b0abf93d22962f4752f3646cd9'
        with = @'
name: assets
path: ${{ runner.temp }}/assets/
if-no-files-found: error
retention-days: 7
'@
    }
)

# What release.yml has to keep (docs/releasing.md): no YAML anchor, alias or
# merge key anywhere, so what a step says is what it runs; no job but sign
# names a secret, a variable or an environment, or mentions the signing key;
# and sign is exactly $SignJob and $SignSteps, with no `${{ }}` in a `run:`
# and none in an `env:` but a plain needs, secrets or vars reference. Throws
# naming the breach.
function Assert-ReleaseWorkflow([string]$Text) {
    $plain = @($Text -split "`n" | Where-Object { $_ -notmatch '^\s*#' }) -join "`n"
    if ($plain -match '(?m)(^\s*-\s+|:\s+)[&*][A-Za-z0-9_-]+|^\s*<<\s*:') { throw 'release.yml uses a YAML anchor, alias or merge key' }
    $workflow = Read-WorkflowJobs $Text
    if (($workflow.Header -join "`n") -match 'secrets\.|vars\.|environment:|TAURI_SIGNING') { throw 'the workflow''s header reaches for a secret' }
    foreach ($name in $workflow.Jobs.Keys) {
        if ($name -eq 'sign') { continue }
        if (($workflow.Jobs[$name].Lines -join "`n") -match 'secrets\.|vars\.|environment:|TAURI_SIGNING') {
            throw "the $name job references a secret, a variable, an environment or the signing key"
        }
    }
    $sign = $workflow.Jobs['sign']
    if (-not $sign) { throw 'no sign job' }
    $head = @($sign.Lines) -join "`n"
    $at = $head.IndexOf("`n    steps:")
    $head = if ($at -ge 0) { $head.Substring(0, $at + "`n    steps:".Length) } else { $head }
    if ($head -cne ($SignJob -replace "`r`n", "`n")) { throw "the sign job's settings are not the ones allowed:`n$head" }
    if ($sign.Steps.Count -ne $SignSteps.Count) { throw "sign has $($sign.Steps.Count) steps, not the $($SignSteps.Count) allowed" }
    for ($i = 0; $i -lt $SignSteps.Count; $i++) {
        $step = $sign.Steps[$i].Keys
        $want = $SignSteps[$i]
        $got = @($step.Keys) -join ','
        if ($got -cne (@($want.Keys) -join ',')) { throw "sign step $($i + 1) has the keys $got, not $(@($want.Keys) -join ',')" }
        foreach ($key in $want.Keys) {
            $value = if ($key -eq 'uses') { $step[$key] -replace '\s+#.*$', '' } else { $step[$key] }
            if ($value -cne ($want[$key] -replace "`r`n", "`n")) { throw "sign step $($i + 1)'s $key is not the one allowed: $value" }
        }
        if ($step['run'] -match '\$\{\{') { throw "sign step $($i + 1) puts an expression in its run" }
        foreach ($line in @($step['env'] -split "`n" | Where-Object { $_ })) {
            if ($line -cnotmatch '^[A-Z_]+: \$\{\{ (needs\.[a-z]+\.outputs\.[a-z]+|secrets\.[A-Z_]+|vars\.[A-Z_]+) \}\}$') {
                throw "sign step $($i + 1) sets $line"
            }
        }
    }
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
        # SemVer 2.0.0 section 11: identifiers one by one, numbers as numbers
        # and below words, the shorter set lower.
        Assert-Equal (Compare-ReleaseVersion '1.2.3-rc.2' '1.2.3-rc.10') -1
        Assert-Equal (Compare-ReleaseVersion '1.2.3-rc.10' '1.2.3-rc.10') 0
        $chain = '1.0.0-alpha', '1.0.0-alpha.1', '1.0.0-alpha.beta', '1.0.0-beta', '1.0.0-beta.2', '1.0.0-beta.11', '1.0.0-rc.1', '1.0.0'
        for ($i = 1; $i -lt $chain.Count; $i++) {
            if ((Compare-ReleaseVersion $chain[$i - 1] $chain[$i]) -ne -1) { throw "$($chain[$i - 1]) should come before $($chain[$i])" }
            if ((Compare-ReleaseVersion $chain[$i] $chain[$i - 1]) -ne 1) { throw "$($chain[$i]) should come after $($chain[$i - 1])" }
        }
    }

    # A CHANGELOG as the repo writes it: the Unreleased section, a version with
    # code fences (headings and a definition shown as text), a nested list and
    # reference links, three versions with subsections, a bare one, an empty
    # one, and the link definitions.
    $changelog = @'
# Changelog

All notable changes are recorded here.

## [Unreleased]

### Fixed

- Not out yet.

## [0.10.2] - 2026-11-10

Tooling notes, with a code block.

### Changed

- A nested list:
  - with a child item
  - and another

  ```sh
  ## example heading
  ### not a heading
  [0.9.10]: nope
  ```

- Run ```pnpm gate``` first: an inline code span, not a fence.
- The [details] are elsewhere; see [the docs][details] and [details][], the
  [spec] and the [notes].

~~~
## another heading, in tildes
```
still inside the tilde fence
~~~

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

[Unreleased]: https://github.com/nrubinmosin/luna/compare/v0.10.2...HEAD
[0.10.2]: https://github.com/nrubinmosin/luna/compare/v0.10.1...v0.10.2
[0.10.1]: https://github.com/nrubinmosin/luna/compare/v0.10.0...v0.10.1
[details]: https://example.com/details "More details"
[Details]: https://example.com/second-definition-is-ignored
[spec]: <https://example.com/spec> 'The spec'
[notes]: https://example.com/notes (Notes)
[unused]: https://example.com/unused
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
        Assert-Equal ((Get-ChangelogVersions $tree) -join ' ') '0.10.2 0.10.1 0.10.0 0.9.11 0.9.10'
    }

    Check 'a heading or a definition inside a code fence is text, not structure' {
        $section = Get-ChangelogSection '0.10.2' $tree
        Assert-Equal $section.Intro 'Tooling notes, with a code block.'
        $expected = @'
Tooling notes, with a code block.

### Changed

- A nested list:
  - with a child item
  - and another

  ```sh
  ## example heading
  ### not a heading
  [0.9.10]: nope
  ```

- Run ```pnpm gate``` first: an inline code span, not a fence.
- The [details] are elsewhere; see [the docs][details] and [details][], the
  [spec] and the [notes].

~~~
## another heading, in tildes
```
still inside the tilde fence
~~~
'@ -replace "`r`n", "`n"
        Assert-Equal $section.Body $expected.Trim()
        # The section after the fences and the inline code span is still found whole.
        Assert-Equal (Get-ChangelogSection '0.10.1' $tree).Intro 'A small release.'
        # The definition shown in the fence is not one: 0.9.10 keeps its real body.
        Assert-Equal (Get-ChangelogSection '0.9.10' $tree).Body '- Fixes and refinements.'
    }

    Check 'the link definitions a lifted section uses come with it, once, as written' {
        # Titles in quotes or parentheses and a <destination> are kept as
        # written; of two definitions of a label (case-insensitive) the first
        # counts; the one shown inside the fence is none.
        $definitions = "[details]: https://example.com/details `"More details`"`n[spec]: <https://example.com/spec> 'The spec'`n[notes]: https://example.com/notes (Notes)`n"
        $notes = New-ReleaseNotes -Version '0.10.2' -Previous 'v0.10.1' -Root $tree
        $tail = "CHANGELOG.md#0102---2026-11-10).`n`n$definitions"
        if (-not $notes.EndsWith($tail)) { throw "the definitions are not appended once at the end, as written:`n$notes" }
        if ($notes -match '\[unused\]|second-definition|\[0\.10\.[12]\]: |\[0\.9\.10\]: https') { throw "a definition nothing refers to was appended:`n$notes" }
        Assert-Equal ([regex]::Matches($notes, '\[details\]: ').Count) 1
        # Definitions go after the verify block too, and still once across several sections.
        $notes = New-ReleaseNotes -Version '0.10.2' -Previous 'v0.9.11' -Checksums "ff84  Luna_0.10.2_x64-setup.exe`n" -Root $tree
        if (-not $notes.EndsWith("``````" + "`n`n$definitions")) { throw "no definitions after the verify block:`n$notes" }
        Assert-Equal ([regex]::Matches($notes, '\[details\]: ').Count) 1
        # Notes without references get none.
        if ((New-ReleaseNotes -Version '0.10.1' -Previous 'v0.10.0' -Root $tree) -match '\]: https') { throw 'a definition appended to notes without references' }
    }

    Check 'a release behind the latest published one is refused' {
        Assert-Throws { New-ReleaseNotes -Version '0.10.0' -Previous 'v0.10.1' -Root $tree } 'v0.10.1, the latest published release, is not older than 0.10.0'
        Assert-Throws { New-ReleaseNotes -Version '0.10.1' -Previous 'v0.10.1' -Root $tree } 'not older than 0.10.1'
        # An older baseline named on purpose renders.
        if ((New-ReleaseNotes -Version '0.10.1' -Previous 'v0.9.11' -Root $tree) -notmatch 'Also from 0\.10\.0') { throw 'an explicit older baseline did not render' }
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
        Assert-Throws { Find-ReleaseArtifacts '0.9.10' -Dir $dir } 'core-fingerprint.txt'
        Set-Content (Join-Path $dir 'core-fingerprint.txt') '0123456789abcdef' -NoNewline
        # No exe: nothing the build made is run, so nothing asks for one.
        $found = Find-ReleaseArtifacts '0.9.10' -Dir $dir
        Assert-Equal (Split-Path -Leaf $found.Setup) 'Luna_0.9.10_x64-setup.exe'
        $found = Find-ReleaseArtifacts '0.9.1' -Dir $dir
        Assert-Equal (Split-Path -Leaf $found.Setup) 'Luna_0.9.1_x64-setup.exe'
        Assert-Equal (@($found.PSObject.Properties.Name) -join ',') 'Setup,Fingerprint'
        Assert-Throws { Find-ReleaseArtifacts '0.9.2' -Dir $dir } 'no installer for 0.9.2'
    }

    Check 'the release source is the commit''s tree, without .git or the skill links' {
        $to = Join-Path $work 'source'
        Export-ReleaseSource 'HEAD' $to
        foreach ($file in 'package.json', 'pnpm-lock.yaml', 'src-tauri/Cargo.lock', 'src-tauri/tauri.conf.json', 'scripts/build-windows.ps1') {
            if (-not (Test-Path -LiteralPath (Join-Path $to $file) -PathType Leaf)) { throw "no $file in the export" }
        }
        foreach ($absent in '.git', '.claude') {
            if (Test-Path -LiteralPath (Join-Path $to $absent)) { throw "$absent in the export" }
        }
        $tracked = git -C $RepoRoot show 'HEAD:package.json'
        Assert-Equal ((Get-Content -LiteralPath (Join-Path $to 'package.json')) -join "`n") ($tracked -join "`n")
        Assert-Equal (Test-Path -LiteralPath "$to.tar") $false
        Assert-Throws { Export-ReleaseSource 'HEAD' $to } 'there already'
    }

    Check 'a build artifact reached through a link is refused' {
        # What a build in a container could leave in the tree it wrote: an
        # artifact, or a folder on the way to it, that is a link elsewhere.
        $root = Join-Path $work 'linked'
        $dir = Join-Path $root 'release'
        $nsis = New-Item -ItemType Directory -Force (Join-Path $dir 'bundle/nsis')
        Set-Content (Join-Path $nsis 'Luna_0.9.10_x64-setup.exe') 'x'
        $outside = New-Item -ItemType Directory -Force (Join-Path $work 'outside')
        Set-Content (Join-Path $outside 'secret.txt') '0123456789abcdef' -NoNewline
        New-Item -ItemType Junction -Path (Join-Path $root 'elsewhere') -Target $outside | Out-Null
        Set-Content (Join-Path $dir 'core-fingerprint.txt') '0123456789abcdef' -NoNewline
        Find-ReleaseArtifacts '0.9.10' -Dir $dir -Root $root | Out-Null
        Assert-Throws { Find-ReleaseArtifacts '0.9.10' -Dir (Join-Path $root 'elsewhere') -Root $root } 'through a link'
        Remove-Item -LiteralPath (Join-Path $dir 'core-fingerprint.txt')
        if ($IsWindows) {
            # A file symlink needs Developer Mode or elevation on Windows; a
            # junction to a folder holding the file stands in for it.
            New-Item -ItemType Junction -Path (Join-Path $dir 'core-fingerprint.txt') -Target $outside | Out-Null
        } else {
            New-Item -ItemType SymbolicLink -Path (Join-Path $dir 'core-fingerprint.txt') -Target (Join-Path $outside 'secret.txt') | Out-Null
        }
        Assert-Throws { Find-ReleaseArtifacts '0.9.10' -Dir $dir -Root $root } 'through a link'
        # The link itself goes, never what it points at.
        [IO.Directory]::Delete((Join-Path $root 'elsewhere'))
        if ($IsWindows) { [IO.Directory]::Delete((Join-Path $dir 'core-fingerprint.txt')) } else { [IO.File]::Delete((Join-Path $dir 'core-fingerprint.txt')) }
        Assert-Equal (Test-Path -LiteralPath (Join-Path $outside 'secret.txt')) $true
    }

    Check 'the signature goes into latest.json only as the build wrote it for the version' {
        $endpoint = Get-UpdaterEndpoint
        $at = [datetime]::new(2026, 11, 7, 12, 30, 5, [DateTimeKind]::Utc)
        $made = { param($sig, $version = '0.9.10', $setup = 'Luna_0.9.10_x64-setup.exe')
            New-UpdaterManifest -Version $version -CoreFingerprint '0123456789abcdef' -Signature $sig -SetupName $setup `
                -Endpoint $endpoint -Notes "Line one`nLine two" -PubDate $at }
        $sign = { param($manifest, $version = '0.9.10', $setup = 'Luna_0.9.10_x64-setup.exe')
            Set-UpdaterManifestSignature -Manifest $manifest -Signature "c2lnbmF0dXJl`n" -Version $version -SetupName $setup -Endpoint $endpoint }
        Assert-Equal (& $sign (& $made '')) (& $made 'c2lnbmF0dXJl')
        Assert-Throws { & $sign (& $made 'old') } 'carries a signature already'
        Assert-Throws { & $sign (& $made '') '0.9.11' } "for version '0.9.10', not 0.9.11"
        Assert-Throws { & $sign (& $made '' '0.9.10' 'Luna_0.9.1_x64-setup.exe') } 'points at'
        Assert-Throws { Set-UpdaterManifestSignature -Manifest (& $made '') -Signature ' ' -Version '0.9.10' -SetupName 'x' -Endpoint $endpoint } 'no signature'
        $json = & $made '' | ConvertFrom-Json -AsHashtable
        $json.platforms['darwin-aarch64'] = $json.platforms['windows-x86_64']
        Assert-Throws { & $sign ($json | ConvertTo-Json -Depth 5) } 'one platform'
        $json = & $made '' | ConvertFrom-Json -AsHashtable
        $json.extra = 1
        Assert-Throws { & $sign ($json | ConvertTo-Json -Depth 5) } 'has the fields'
        $json = & $made '' | ConvertFrom-Json -AsHashtable
        $json.pub_date = 'soon'
        Assert-Throws { & $sign ($json | ConvertTo-Json -Depth 5) } 'not a date'
    }

    Check 'the Tauri CLI to sign with is the one pnpm-lock.yaml pins' {
        $cli = Get-LockedNpmPackage '@tauri-apps/cli-linux-x64-gnu'
        $wrapper = Get-LockedNpmPackage '@tauri-apps/cli'
        Assert-Equal $cli.Version $wrapper.Version
        Assert-Equal $cli.Url "https://registry.npmjs.org/@tauri-apps/cli-linux-x64-gnu/-/cli-linux-x64-gnu-$($cli.Version).tgz"
        if ($cli.Integrity -notmatch '^sha512-[A-Za-z0-9+/]{86}==$') { throw "integrity $($cli.Integrity)" }
        $root = New-Item -ItemType Directory -Force (Join-Path $work 'lock')
        $entry = "  '@x/y@1.0.0':`n    resolution: {integrity: sha512-AAAA}`n"
        Set-Content (Join-Path $root 'pnpm-lock.yaml') "packages:`n`n$entry`nsnapshots:`n`n  '@x/y@1.0.0': {}`n" -NoNewline
        Assert-Equal (Get-LockedNpmPackage '@x/y' $root).Version '1.0.0'
        Set-Content (Join-Path $root 'pnpm-lock.yaml') "packages:`n`n$entry`n$($entry -replace '1\.0\.0', '1.0.1')" -NoNewline
        Assert-Throws { Get-LockedNpmPackage '@x/y' $root } 'not 2'
        Set-Content (Join-Path $root 'pnpm-lock.yaml') "packages:`n`n  '@x/y@1.0.0':`n    resolution: {integrity: sha512-AAAA, tarball: https://example.com/y.tgz}`n" -NoNewline
        Assert-Throws { Get-LockedNpmPackage '@x/y' $root } 'not 0'
    }

    Check 'a download is held to its sha512 integrity' {
        $file = Join-Path (New-Item -ItemType Directory -Force (Join-Path $work 'sri')) 'cli.tgz'
        [IO.File]::WriteAllBytes($file, [Text.Encoding]::ASCII.GetBytes('TAURI'))
        $good = 'sha512-' + [Convert]::ToBase64String([Security.Cryptography.SHA512]::HashData([Text.Encoding]::ASCII.GetBytes('TAURI')))
        Assert-Sha512Integrity $file $good
        $bad = 'sha512-' + [Convert]::ToBase64String([Security.Cryptography.SHA512]::HashData([Text.Encoding]::ASCII.GetBytes('TAURJ')))
        Assert-Throws { Assert-Sha512Integrity $file $bad } 'does not match'
        Assert-Throws { Assert-Sha512Integrity $file 'sha256-AAAA' } 'not a sha512 integrity'
    }

    Check 'only the named file comes out of a tarball' {
        $dir = New-Item -ItemType Directory -Force (Join-Path $work 'tar')
        $tarball = Join-Path $dir 'pkg.tgz'
        $out = [IO.File]::Create($tarball)
        $writer = [System.Formats.Tar.TarWriter]::new([IO.Compression.GZipStream]::new($out, [IO.Compression.CompressionMode]::Compress))
        foreach ($name in 'package/package.json', '../escape.node', 'package/cli.node') {
            $entry = [System.Formats.Tar.PaxTarEntry]::new([System.Formats.Tar.TarEntryType]::RegularFile, $name)
            $entry.DataStream = [IO.MemoryStream]::new([Text.Encoding]::ASCII.GetBytes("data of $name"))
            $writer.WriteEntry($entry)
        }
        $writer.Dispose()
        $to = Join-Path $dir 'cli.node'
        Export-TarballEntry $tarball 'package/cli.node' $to
        Assert-Equal (Get-Content -LiteralPath $to -Raw) 'data of package/cli.node'
        Assert-Equal (@(Get-ChildItem -LiteralPath $dir).Name -join ',') 'cli.node,pkg.tgz'
        Assert-Equal (Test-Path -LiteralPath (Join-Path $work 'escape.node')) $false
        Assert-Throws { Export-TarballEntry $tarball 'package/cli.node' $to } 'already exists'
        Assert-Throws { Export-TarballEntry $tarball 'package/missing.node' (Join-Path $dir 'm') } 'has no package/missing.node'
    }

    # A tree as the sign job checks it out, its lockfile pinning a stand-in
    # for the CLI's tarball, and a bundle as the build leaves it. Docker and
    # the network are stood in for by $Run and $Fetch (Invoke-ReleaseSign).
    $signFixture = {
        param([string]$Name)
        $root = Join-Path $work $Name
        New-Item -ItemType Directory -Force (Join-Path $root 'src-tauri') | Out-Null
        Copy-Item -LiteralPath (Join-Path $RepoRoot 'src-tauri/tauri.conf.json') -Destination (Join-Path $root 'src-tauri')
        $tarball = Join-Path $root 'good.tgz'
        $out = [IO.File]::Create($tarball)
        $writer = [System.Formats.Tar.TarWriter]::new([IO.Compression.GZipStream]::new($out, [IO.Compression.CompressionMode]::Compress))
        $entry = [System.Formats.Tar.PaxTarEntry]::new([System.Formats.Tar.TarEntryType]::RegularFile, 'package/cli.linux-x64-gnu.node')
        $entry.DataStream = [IO.MemoryStream]::new([Text.Encoding]::ASCII.GetBytes('the native module'))
        $writer.WriteEntry($entry)
        $writer.Dispose()
        $integrity = 'sha512-' + [Convert]::ToBase64String([Security.Cryptography.SHA512]::HashData([IO.File]::ReadAllBytes($tarball)))
        Set-Content (Join-Path $root 'pnpm-lock.yaml') "packages:`n`n  '@tauri-apps/cli-linux-x64-gnu@2.12.3':`n    resolution: {integrity: $integrity}`n" -NoNewline
        $bundle = New-Item -ItemType Directory -Force (Join-Path $root 'bundle')
        Set-Content (Join-Path $bundle 'Luna_0.9.10_x64-setup.exe') 'setup' -NoNewline
        New-UpdaterManifest -Version '0.9.10' -CoreFingerprint '0123456789abcdef' -Signature '' -SetupName 'Luna_0.9.10_x64-setup.exe' `
            -Endpoint (Get-UpdaterEndpoint $root) | Set-Content (Join-Path $bundle 'latest.json') -Encoding utf8
        $key = Join-Path $root 'updater.key'
        Set-Content $key 'not a key' -NoNewline
        [pscustomobject]@{ Root = $root; Tarball = $tarball; Bundle = $bundle.FullName; Key = $key; Calls = [System.Collections.Generic.List[object]]::new() }
    }
    # The stand-in for docker: records each run, and for the sign run checks
    # the native module is the tarball's and writes a .sig.
    $fakeRun = {
        param($f, [bool]$FailCheck = $false)
        {
            param($What, $DockerArgs)
            $f.Calls.Add([pscustomobject]@{ What = $What; Args = @($DockerArgs) })
            $cliMount = $DockerArgs[[array]::IndexOf($DockerArgs, '-v') + 1] -replace ':/cli:ro$', ''
            if ((Get-Content -LiteralPath (Join-Path $cliMount 'cli.node') -Raw) -cne 'the native module') { throw 'not the module' }
            if ($What -eq 'tauri signer sign') { Set-Content (Join-Path $f.Bundle 'Luna_0.9.10_x64-setup.exe.sig') 'U0lHTkFUVVJF' -NoNewline }
            elseif ($FailCheck) { throw "$What failed (exit 1)" }
        }.GetNewClosure()
    }

    Check 'a CLI download that does not match the lockfile is never run' {
        $f = & $signFixture 'sign-mismatch'
        $before = Get-Content -LiteralPath (Join-Path $f.Bundle 'latest.json') -Raw
        Assert-Throws {
            Invoke-ReleaseSign -Dir $f.Bundle -Version '0.9.10' -UpdaterKey $f.Key -Root $f.Root `
                -Fetch { param($url, $to) Set-Content $to 'a tampered tarball' } -Run (& $fakeRun $f)
        } 'does not match'
        Assert-Equal $f.Calls.Count 0
        Assert-Equal (Test-Path -LiteralPath (Join-Path $f.Bundle 'Luna_0.9.10_x64-setup.exe.sig')) $false
        Assert-Equal (Get-Content -LiteralPath (Join-Path $f.Bundle 'latest.json') -Raw) $before
        Assert-Equal (@(Get-ChildItem -LiteralPath (Join-Path $f.Root 'src-tauri/target/release-signer') -ErrorAction SilentlyContinue).Count) 0
    }

    Check 'signing runs the checked CLI isolated, then checks against the updater''s key' {
        $f = & $signFixture 'sign-ok'
        Invoke-ReleaseSign -Dir $f.Bundle -Version '0.9.10' -UpdaterKey $f.Key -Root $f.Root `
            -Fetch { param($url, $to) Copy-Item -LiteralPath $f.Tarball -Destination $to }.GetNewClosure() -Run (& $fakeRun $f)
        Assert-Equal (@($f.Calls.What) -join ',') 'tauri signer sign,the signature check'
        foreach ($call in $f.Calls) {
            $text = $call.Args -join ' '
            foreach ($flag in '--rm', '--network none', '--read-only', '--cap-drop ALL', '--security-opt no-new-privileges') {
                if (-not $text.Contains($flag)) { throw "$($call.What) runs without $flag" }
            }
        }
        $sign = $f.Calls[0].Args -join ' '
        if (-not $sign.Contains('TAURI_SIGNING_PRIVATE_KEY_PATH=/keys/updater.key') -or -not $sign.Contains(':/keys/updater.key:ro')) { throw 'the key is not mounted for signing' }
        $check = $f.Calls[1].Args
        if (($check -join ' ') -match 'keys|TAURI_SIGNING') { throw 'the check is handed the key' }
        Assert-Equal $check[[array]::IndexOf($check, 'verify') + 3] (Get-UpdaterPubkey $f.Root)
        $manifest = Get-Content -LiteralPath (Join-Path $f.Bundle 'latest.json') -Raw | ConvertFrom-Json
        Assert-Equal $manifest.platforms.'windows-x86_64'.signature 'U0lHTkFUVVJF'
        Assert-Equal (@(Get-ChildItem -LiteralPath (Join-Path $f.Root 'src-tauri/target/release-signer')).Count) 0
    }

    Check 'a rotation''s bridge release is checked against the trusted key, not the one it carries' {
        $f = & $signFixture 'sign-bridge'
        Invoke-ReleaseSign -Dir $f.Bundle -Version '0.9.10' -UpdaterKey $f.Key -Root $f.Root -TrustedPubkey 'T0xEIEtFWQ==' `
            -Fetch { param($url, $to) Copy-Item -LiteralPath $f.Tarball -Destination $to }.GetNewClosure() -Run (& $fakeRun $f)
        $check = $f.Calls[1].Args
        Assert-Equal $check[[array]::IndexOf($check, 'verify') + 3] 'T0xEIEtFWQ=='
    }

    Check 'a signature that fails the check is taken away again' {
        $f = & $signFixture 'sign-fail'
        $fetch = { param($url, $to) Copy-Item -LiteralPath $f.Tarball -Destination $to }.GetNewClosure()
        $run = & $fakeRun $f $true
        Assert-Throws {
            Invoke-ReleaseSign -Dir $f.Bundle -Version '0.9.10' -UpdaterKey $f.Key -Root $f.Root -Fetch $fetch -Run $run
        } 'the signature check failed'
        Assert-Equal (Test-Path -LiteralPath (Join-Path $f.Bundle 'Luna_0.9.10_x64-setup.exe.sig')) $false
        Assert-Equal ((Get-Content -LiteralPath (Join-Path $f.Bundle 'latest.json') -Raw | ConvertFrom-Json).platforms.'windows-x86_64'.signature) ''
    }

    Check 'the bundle is held to the build''s checksums, file for file' {
        $dir = New-Item -ItemType Directory -Force (Join-Path $work 'sums')
        Set-Content (Join-Path $dir 'Luna_0.9.10_x64-setup.exe') 'setup' -NoNewline
        Set-Content (Join-Path $dir 'latest.json') '{}' -NoNewline
        $sha = { param($text) [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData([Text.Encoding]::UTF8.GetBytes($text))).ToLowerInvariant() }
        $sums = "$(& $sha 'setup')  Luna_0.9.10_x64-setup.exe`n$(& $sha '{}')  latest.json`n"
        Assert-AssetChecksums $dir $sums
        Assert-AssetChecksums $dir ($sums -replace '  latest', ' *latest')
        Set-Content (Join-Path $dir 'latest.json') '{"x":1}' -NoNewline
        Assert-Throws { Assert-AssetChecksums $dir $sums } 'latest.json has sha256'
        Set-Content (Join-Path $dir 'latest.json') '{}' -NoNewline
        Set-Content (Join-Path $dir 'Luna_0.9.10_x64-setup.exe.sig') 'x'
        Assert-Throws { Assert-AssetChecksums $dir $sums } 'do not list: Luna_0.9.10_x64-setup.exe.sig'
        Remove-Item -LiteralPath (Join-Path $dir 'Luna_0.9.10_x64-setup.exe.sig')
        Remove-Item -LiteralPath (Join-Path $dir 'latest.json')
        Assert-Throws { Assert-AssetChecksums $dir $sums } 'latest.json is missing'
        Assert-Throws { Assert-AssetChecksums $dir '' } 'no checksums'
        Assert-Throws { Assert-AssetChecksums $dir "$(& $sha 'setup')  ../setup.exe" } 'not a sha256sum line'
    }

    Check 'release.yml keeps the key in the sign job, and the sign job as allowed' {
        $text = [IO.File]::ReadAllText((Join-Path $RepoRoot '.github/workflows/release.yml')) -replace "`r`n", "`n"
        $workflow = Read-WorkflowJobs $text
        Assert-Equal ($workflow.Jobs.Keys -join ',') 'verify,build,sign,sbom,attest,publish'
        Assert-ReleaseWorkflow $text

        # Each breach, as an edit of the real file.
        $edit = { param($from, $to) if (-not $text.Contains($from)) { throw "no '$from' in release.yml" }; $text.Replace($from, $to) }
        $beforeSign = '      # The key goes to this step alone'
        $signRun = '        run: ./scripts/release-sign.ps1 -Dir "$env:RUNNER_TEMP/assets" -Version $env:VERSION -RequireTrustedPubkey'
        Assert-Throws { Assert-ReleaseWorkflow (& $edit $beforeSign "      - name: Install`n        run: |`n          echo ready`n          pnpm install --frozen-lockfile`n$beforeSign") } 'sign has 6 steps'
        Assert-Throws { Assert-ReleaseWorkflow (& $edit $beforeSign "      - uses: actions/setup-node@0123456789abcdef0123456789abcdef01234567`n$beforeSign") } 'sign has 6 steps'
        Assert-Throws { Assert-ReleaseWorkflow (& $edit $signRun "$signRun`n          pnpm install") } 'run is not the one allowed'
        Assert-Throws { Assert-ReleaseWorkflow (& $edit $signRun "        run: `${{ format('{0}{1} install', 'pn', 'pm') }}") } 'run is not the one allowed'
        Assert-Throws { Assert-ReleaseWorkflow (& $edit $signRun ($signRun -replace ' -RequireTrustedPubkey', '')) } 'run is not the one allowed'
        Assert-Throws { Assert-ReleaseWorkflow (& $edit $signRun '        run: *command') } 'anchor, alias'
        Assert-Throws { Assert-ReleaseWorkflow (& $edit '      - name: Sign the installer' '      - name: &step Sign the installer') } 'anchor, alias'
        Assert-Throws { Assert-ReleaseWorkflow (& $edit "        if: needs.verify.outputs.signed == 'true'" "        <<: *defaults`n        if: needs.verify.outputs.signed == 'true'") } 'anchor, alias'
        Assert-Throws {
            Assert-ReleaseWorkflow (& $edit '          VERSION: ${{ needs.verify.outputs.version }}' "          VERSION: `${{ needs.verify.outputs.version }}`n          BASH_ENV: `${{ format('x') }}")
        } 'env is not the one allowed'
        Assert-Throws { Assert-ReleaseWorkflow (& $edit "          sparse-checkout-cone-mode: false`n" '') } 'with is not the one allowed'
        Assert-Throws { Assert-ReleaseWorkflow (& $edit "    timeout-minutes: 15`n    environment:" "    timeout-minutes: 15`n    container: node:24`n    environment:") } 'settings are not the ones allowed'
        Assert-Throws { Assert-ReleaseWorkflow (& $edit "    environment: `${{ needs.verify.outputs.signed == 'true' && 'release' || '' }}`n" '') } 'settings are not the ones allowed'
        Assert-Throws {
            Assert-ReleaseWorkflow (& $edit '        run: ./scripts/build-windows.ps1 -Installer' "        env:`n          KEY: `${{ secrets.TAURI_SIGNING_PRIVATE_KEY }}`n        run: ./scripts/build-windows.ps1 -Installer")
        } 'the build job references a secret'
        Assert-Throws { Assert-ReleaseWorkflow (& $edit "  build:`n    needs: verify`n" "  build:`n    needs: verify`n    environment: release`n") } 'the build job references'
    }

    Check 'the trusted public key is the embedded one unless a key is given, and required where asked' {
        $pubkey = Get-UpdaterPubkey
        Assert-Equal (Resolve-TrustedPubkey '') $null
        Assert-Equal (Resolve-TrustedPubkey 'embedded') $null
        Assert-Equal (Resolve-TrustedPubkey 'embedded' -Required) $null
        Assert-Equal (Resolve-TrustedPubkey " $pubkey`n") $pubkey
        Assert-Equal (Resolve-TrustedPubkey $pubkey -Required) $pubkey
        # Empty where it is required: the `release` environment does not
        # define the variable, so a repository-level one would have stood in.
        Assert-Throws { Resolve-TrustedPubkey '' -Required } 'UPDATER_TRUSTED_PUBKEY'
        Assert-Throws { Resolve-TrustedPubkey 'yes' } 'not a minisign public key'
        Assert-Throws { Resolve-TrustedPubkey 'dW50cnVzdGVkIGNvbW1lbnQ6IHgKUldRCg==' } 'not a minisign public key'
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
