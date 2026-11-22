#Requires -Version 7.4
# Tests for release-lib.ps1, on copies of the repo's own version files in a
# folder of their own under test-build/.
# Run: pwsh scripts/release-lib.test.ps1 (the gate runs it with the scripts group)
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

# A fresh tree holding copies of the repo's version files, and of
# updater.json, whose endpoint names the repo.
function New-Tree([string]$Name) {
    $root = Join-Path $work $Name
    foreach ($file in @(Get-ReleaseVersionFiles) + $UpdaterSource) {
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
/src-tauri/updater.json
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
        name = 'Check the bundle against its checksums'
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
        name = 'Sign a copy with a throwaway key'
        if   = 'needs.verify.outputs.signed != ''true'''
        env  = 'VERSION: ${{ needs.verify.outputs.version }}'
        run  = @'
|
Copy-Item -Recurse "$env:RUNNER_TEMP/assets" "$env:RUNNER_TEMP/throwaway"
./scripts/release-sign.ps1 -Dir "$env:RUNNER_TEMP/throwaway" -Version $env:VERSION -Throwaway
Remove-Item -Recurse -Force "$env:RUNNER_TEMP/throwaway"
'@
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
    $current = (Find-VersionLine ([IO.File]::ReadAllText((Join-Path $RepoRoot 'src-tauri/Cargo.toml'))) 'cargo-toml').Groups[2].Value

    Check 'the repo''s version files agree' {
        Assert-Equal (Get-ReleaseVersion) $current
    }

    Check 'the version files are Cargo.toml and Cargo.lock, and a tree needs both' {
        $root = New-Tree cargo-only
        Assert-Equal ((Get-ReleaseVersionFiles $root) -join ',') 'src-tauri/Cargo.toml,src-tauri/Cargo.lock'
        Set-ReleaseVersion '7.8.9' $root
        Assert-Equal (Get-ReleaseVersion $root) '7.8.9'
        Remove-Item -LiteralPath (Join-Path $root 'src-tauri/Cargo.lock')
        Assert-Throws { Get-ReleaseVersion $root } 'no src-tauri/Cargo.lock'
        Assert-Throws { Set-ReleaseVersion '7.8.10' $root } 'no src-tauri/Cargo.lock'
        Assert-Equal (Find-VersionLine ([IO.File]::ReadAllText((Join-Path $root 'src-tauri/Cargo.toml'))) 'cargo-toml').Groups[2].Value '7.8.9'
    }

    Check 'a bump changes the version line of each file and nothing else' {
        $root = New-Tree bump
        $before = @{}
        foreach ($file in Get-ReleaseVersionFiles $root) { $before[$file] = [IO.File]::ReadAllText((Join-Path $root $file)) }
        Set-ReleaseVersion '12.34.56-rc.1' $root
        Assert-Equal (Get-ReleaseVersion $root) '12.34.56-rc.1'
        foreach ($file in Get-ReleaseVersionFiles $root) {
            $changed = @(Compare-Lines $before[$file] ([IO.File]::ReadAllText((Join-Path $root $file))))
            if ($changed.Count -ne 1 -or $changed[0] -notmatch '12\.34\.56-rc\.1') { throw "$file changed in: $($changed -join ' | ')" }
        }
    }

    Check 'a bump keeps CRLF line endings' {
        $root = New-Tree crlf
        foreach ($file in 'src-tauri/Cargo.toml', 'src-tauri/Cargo.lock') {
            $path = Join-Path $root $file
            [IO.File]::WriteAllText($path, ([IO.File]::ReadAllText($path) -replace "`r?`n", "`r`n"))
        }
        Set-ReleaseVersion '2.0.0' $root
        Assert-Equal (Get-ReleaseVersion $root) '2.0.0'
        foreach ($file in 'src-tauri/Cargo.toml', 'src-tauri/Cargo.lock') {
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

# The SBOM attested against the installer.
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
        foreach ($name in 'Luna_0.9.10_x64-setup.exe', 'rm-luna_0.9.1_x64-setup.exe', 'rm-luna_0.9.1_x64-setup.exe.sig', 'rm-luna_0.9.10_x64-setup.exe') {
            Set-Content (Join-Path $nsis $name) 'x'
        }
        Set-Content (Join-Path $nsis 'Luna_0.9.2_x64-setup.exe') 'x'
        Assert-Throws { Find-ReleaseArtifacts '0.9.10' -Dir $dir } 'core-fingerprint.txt'
        Set-Content (Join-Path $dir 'core-fingerprint.txt') '0123456789abcdef' -NoNewline
        # No exe: nothing the build made is run, so nothing asks for one.
        $found = Find-ReleaseArtifacts '0.9.10' -Dir $dir
        Assert-Equal (Split-Path -Leaf $found.Setup) 'rm-luna_0.9.10_x64-setup.exe'
        $found = Find-ReleaseArtifacts '0.9.1' -Dir $dir
        Assert-Equal (Split-Path -Leaf $found.Setup) 'rm-luna_0.9.1_x64-setup.exe'
        Assert-Equal (@($found.PSObject.Properties.Name) -join ',') 'Setup,Fingerprint'
        Assert-Throws { Find-ReleaseArtifacts '0.9.2' -Dir $dir } 'no installer for 0.9.2'
    }

    Check 'the release source is the commit''s tree, without .git or the skill links' {
        $to = Join-Path $work 'source'
        Export-ReleaseSource 'HEAD' $to
        foreach ($file in 'src-tauri/Cargo.toml', 'src-tauri/Cargo.lock', 'src-tauri/updater.json', 'scripts/release-lib.ps1') {
            if (-not (Test-Path -LiteralPath (Join-Path $to $file) -PathType Leaf)) { throw "no $file in the export" }
        }
        foreach ($absent in '.git', '.claude') {
            if (Test-Path -LiteralPath (Join-Path $to $absent)) { throw "$absent in the export" }
        }
        # An ASCII file: git's output reaches pwsh in the console's encoding.
        $tracked = git -C $RepoRoot show 'HEAD:src-tauri/updater.json'
        Assert-Equal ((Get-Content -LiteralPath (Join-Path $to 'src-tauri/updater.json')) -join "`n") ($tracked -join "`n")
        Assert-Equal (Test-Path -LiteralPath "$to.tar") $false
        Assert-Throws { Export-ReleaseSource 'HEAD' $to } 'there already'
    }

    Check 'a build artifact reached through a link is refused' {
        # What a build in a container could leave in the tree it wrote: an
        # artifact, or a folder on the way to it, that is a link elsewhere.
        $root = Join-Path $work 'linked'
        $dir = Join-Path $root 'release'
        $nsis = New-Item -ItemType Directory -Force (Join-Path $dir 'bundle/nsis')
        Set-Content (Join-Path $nsis 'rm-luna_0.9.10_x64-setup.exe') 'x'
        $outside = New-Item -ItemType Directory -Force (Join-Path $work 'outside')
        Set-Content (Join-Path $outside 'secret.txt') '0123456789abcdef' -NoNewline
        # A link as each OS makes one without privileges: a junction on
        # Windows (a symlink there needs Developer Mode or elevation), so a
        # folder holding the file stands in for a link to the file; a symlink
        # elsewhere, where pwsh makes no junction and says nothing about it.
        # Checked to be there, so a link never made cannot pass for refused.
        $link = {
            param($path, $folder, $file)
            if ($IsWindows) { New-Item -ItemType Junction -Path $path -Target $folder | Out-Null }
            else { New-Item -ItemType SymbolicLink -Path $path -Target $(if ($file) { $file } else { $folder }) | Out-Null }
            if (-not (Test-Link $path)) { throw "no link made at $path" }
        }
        # The link itself goes, never what it points at: a junction or a
        # folder symlink is removed as a folder on Windows, unlinked elsewhere.
        $unlink = { param($path) if ($IsWindows) { [IO.Directory]::Delete($path) } else { [IO.File]::Delete($path) } }
        & $link (Join-Path $root 'elsewhere') $outside
        Set-Content (Join-Path $dir 'core-fingerprint.txt') '0123456789abcdef' -NoNewline
        Find-ReleaseArtifacts '0.9.10' -Dir $dir -Root $root | Out-Null
        Assert-Throws { Find-ReleaseArtifacts '0.9.10' -Dir (Join-Path $root 'elsewhere') -Root $root } 'through a link'
        Remove-Item -LiteralPath (Join-Path $dir 'core-fingerprint.txt')
        & $link (Join-Path $dir 'core-fingerprint.txt') $outside (Join-Path $outside 'secret.txt')
        Assert-Throws { Find-ReleaseArtifacts '0.9.10' -Dir $dir -Root $root } 'through a link'
        & $unlink (Join-Path $root 'elsewhere')
        & $unlink (Join-Path $dir 'core-fingerprint.txt')
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

    Check 'the updater''s endpoint and key are read from updater.json, which the updater compiles in' {
        $source = Read-UpdaterSource
        Assert-Equal (Get-UpdaterEndpoint) $source.Endpoint
        Assert-Equal (Get-UpdaterPubkey) $source.Pubkey
        Assert-Equal (Get-RepoUrl) 'https://github.com/nrubinmosin/luna'
        Assert-Equal (Resolve-TrustedPubkey $source.Pubkey) $source.Pubkey
        # The one file both read: desktop/updater.rs takes its endpoint and key
        # from it and no other, and its own test holds the two to it.
        $updater = [IO.File]::ReadAllText((Join-Path $RepoRoot 'src-tauri/src/desktop/updater.rs')) -replace "`r`n", "`n"
        $updater = $updater.Substring(0, $updater.IndexOf("`n#[cfg(test)]`npub(crate) mod tests {"))
        Assert-Equal ([regex]::Matches($updater, 'include_str!\("\.\./\.\./updater\.json"\)').Count) 1
        $root = New-Item -ItemType Directory -Force (Join-Path $work 'source/src-tauri')
        Set-Content (Join-Path $root 'updater.json') '{"endpoint":"https://github.com/a/b/releases/latest/download/latest.json","pubkey":"x","extra":1}'
        Assert-Throws { Read-UpdaterSource (Split-Path $root) } 'endpoint and pubkey, and nothing else'
        Set-Content (Join-Path $root 'updater.json') '{"endpoint":"","pubkey":"x"}'
        Assert-Throws { Read-UpdaterSource (Split-Path $root) } 'has no endpoint'
    }

    # A signature in the signing's form, its signatures zeros: the form check
    # reads the lines, never the cryptography (OpenSSL checks that).
    $formed = {
        param([string]$Pubkey, [string]$Comment, [string]$Algorithm = 'ED', [int]$Global = 64)
        $key = [Convert]::FromBase64String(([Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($Pubkey)) -split '\r?\n')[1])
        $bin = [byte[]]([Text.Encoding]::ASCII.GetBytes($Algorithm) + $key[2..9] + [byte[]]::new(64))
        $file = "untrusted comment: signature from tauri secret key`n$([Convert]::ToBase64String($bin))`ntrusted comment: $Comment`n$([Convert]::ToBase64String([byte[]]::new($Global)))`n"
        [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($file))
    }
    $ourComment = "timestamp:1795000000`tfile:Luna_0.9.10_x64-setup.exe`tversion:0.9.10`tcore_fingerprint:0123456789abcdef"
    $fixtures = Join-Path $RepoRoot 'src-tauri/fixtures/updater'
    $fixture = { param($name) (Get-Content -LiteralPath (Join-Path $fixtures $name) -Raw).Trim() }
    # Keys the Tauri CLI made, with no password (the maintainer's key's form)
    # and with one (fixtures/updater/README.md).
    $fixtureKey = Join-Path $fixtures 'tauri.key'
    $fixturePub = & $fixture 'tauri.key.pub'
    $passwordKey = & $fixture 'tauri-password.key'
    $passwordPub = & $fixture 'tauri-password.key.pub'
    $password = 'luna fixture password'
    # One minisign 0.10, the crate the Tauri CLI makes keys with, made with a
    # password beyond ASCII and beyond any ANSI code page: Latin, a dash,
    # Cyrillic and an emoji, as escapes (fixtures/updater/README.md).
    $unicodeKey = & $fixture 'tauri-unicode.key'
    $unicodePub = & $fixture 'tauri-unicode.key.pub'
    $unicodePassword = "L`u{FC}na `u{2014} `u{43A}`u{43B}`u{44E}`u{447} `u{1F511} fixture"
    # What the release's signing made with tauri.key over setup.bin as
    # Luna_0.20.0_x64-setup.exe for 0.20.0 and the core 0123456789abcdef,
    # dated $signedAt; and the signature tauri-plugin-updater 2.14.1 itself
    # took, made with the same key and date by minisign's signer, which hedges
    # its nonces (fixtures/updater/README.md).
    $openSslSig = & $fixture 'release-0.20.0.openssl.sig'
    $pluginSig = & $fixture 'release-0.20.0.sig'
    $signedAt = { [DateTimeOffset]::FromUnixTimeSeconds(1795158621) }
    $openssl = Get-OpenSsl -ForKey
    # setup.bin under the name a release gives its installer, in a folder of its own.
    $setupCopy = {
        param([string]$Name)
        $dir = New-Item -ItemType Directory -Force (Join-Path $work $Name)
        $path = Join-Path $dir 'Luna_0.20.0_x64-setup.exe'
        Copy-Item -LiteralPath (Join-Path $fixtures 'setup.bin') -Destination $path
        $path
    }

    Check 'the key is handled by the system''s OpenSSL 3 alone, found with no PATH and no process run' {
        if ($IsWindows) {
            # Git for Windows' install, in either registry view, as the signing reads it.
            $installs = foreach ($view in [Microsoft.Win32.RegistryView]::Registry64, [Microsoft.Win32.RegistryView]::Registry32) {
                $base = [Microsoft.Win32.RegistryKey]::OpenBaseKey([Microsoft.Win32.RegistryHive]::LocalMachine, $view)
                $key = $base.OpenSubKey('SOFTWARE\GitForWindows')
                if ($key) { $key.GetValue('InstallPath'); $key.Dispose() }
                $base.Dispose()
            }
            $under = @($installs | Where-Object { $_ -and $openssl.StartsWith((Join-Path $_ ''), [StringComparison]::OrdinalIgnoreCase) })
            if (-not $under) { throw "signing takes $openssl, not one under Git for Windows' $($installs -join ' or ')" }
        } else {
            Assert-Equal $openssl '/usr/bin/openssl'
        }
        if ("$(& $openssl version)" -cnotmatch '^OpenSSL 3\.') { throw "$openssl is not OpenSSL 3" }
        # Found the same with nothing on PATH, and with git, were it run,
        # failing the test: the lookup starts no process but OpenSSL's own.
        function git { throw 'git was run' }
        $path = $env:PATH
        try {
            $env:PATH = ''
            Assert-Equal (Get-OpenSsl -ForKey) $openssl
        } finally {
            $env:PATH = $path
        }
    }

    Check 'BLAKE2b is RFC 7693''s, the 512-bit one as OpenSSL makes it' {
        $hex = { param([byte[]]$b) [Convert]::ToHexString($b).ToLowerInvariant() }
        Assert-Equal (& $hex (Get-Blake2b ([Text.Encoding]::ASCII.GetBytes('abc')) 64)) 'ba80a53f981c4d0d6a2797b69f12f6e94c212f14685ac4b74b12bb6fdbffa2d17d87c5392aab792dc252d5de4533cc9518d38aa8dbf1925ab92386edd4009923'
        Assert-Equal (& $hex (Get-Blake2b ([byte[]]@()) 32)) '0e5751c026e543b2e8ab2eb06099daa1d1e5df47778f7787faab45cdf12fe3a8'
        $dir = New-Item -ItemType Directory -Force (Join-Path $work 'blake2b')
        foreach ($length in 0, 1, 127, 128, 129, 300) {
            $data = [byte[]]::new($length)
            for ($i = 0; $i -lt $length; $i++) { $data[$i] = ($i * 7 + 3) % 256 }
            $path = Join-Path $dir "$length.bin"
            [IO.File]::WriteAllBytes($path, $data)
            Assert-Equal (& $hex (Get-Blake2b $data 64)) (& $hex (Invoke-OpenSsl $openssl 'hashing' @('dgst', '-blake2b512', '-binary', $path)))
        }
        Assert-Throws { Get-Blake2b ([byte[]]@()) 65 } '1 to 64 bytes'
    }

    Check 'a key the Tauri CLI made opens, with its password or none, and anything else is refused in fixed words' {
        $key = Read-UpdaterSecretKey $openssl (Get-Content -LiteralPath $fixtureKey -Raw) ''
        $pub = Read-UpdaterPubkey $fixturePub
        Assert-Equal ([Convert]::ToHexString($key.KeyId)) ([Convert]::ToHexString($pub.KeyId))
        Assert-Equal ([Convert]::ToHexString($key.Public)) ([Convert]::ToHexString($pub.Public))
        Assert-Equal $key.Seed.Length 32
        $withPassword = Read-UpdaterSecretKey $openssl $passwordKey $password
        Assert-Equal ([Convert]::ToHexString($withPassword.Public)) ([Convert]::ToHexString((Read-UpdaterPubkey $passwordPub).Public))
        # A password beyond ASCII is the UTF-8 bytes minisign derives the key
        # from: not another normalization of the text, nor what the ANSI code
        # page would make of it.
        $unicode = Read-UpdaterSecretKey $openssl $unicodeKey $unicodePassword
        Assert-Equal ([Convert]::ToHexString($unicode.Public)) ([Convert]::ToHexString((Read-UpdaterPubkey $unicodePub).Public))
        Assert-Equal ([Convert]::ToHexString($unicode.KeyId)) ([Convert]::ToHexString((Read-UpdaterPubkey $unicodePub).KeyId))
        $ansi = [Text.Encoding]::GetEncoding(1252).GetString([Text.Encoding]::GetEncoding(1252).GetBytes($unicodePassword))
        foreach ($wrong in $unicodePassword.Normalize([Text.NormalizationForm]::FormD), $ansi, $unicodePassword.Substring(0, 6)) {
            Assert-Throws { Read-UpdaterSecretKey $openssl $unicodeKey $wrong } 'the updater key does not open with the password given'
        }
        # What OpenSSL could not be handed whole on its input is refused, in
        # fixed words.
        foreach ($cut in "$password`n", "a`rb", "a`tb", "a$([char]0x1A)b", "a$([char]0)b") {
            try { Read-UpdaterSecretKey $openssl $passwordKey $cut; throw 'took it' } catch {
                Assert-Equal "$_" 'the updater key''s password has a line break or another control character, which OpenSSL cannot be handed on its input'
            }
        }
        Assert-Throws { Read-UpdaterSecretKey $openssl $passwordKey ('x' * 1024) } 'longer than the 1023 bytes'
        Assert-Throws { Read-UpdaterSecretKey $openssl $passwordKey "a$([char]0xD800)b" } 'not valid Unicode'
        # A key file with a line break after it, as an editor saves one.
        Read-UpdaterSecretKey $openssl "$(Get-Content -LiteralPath $fixtureKey -Raw)`n" '' | Out-Null
        foreach ($wrong in '', 'luna fixture password ', 'LUNA FIXTURE PASSWORD') {
            Assert-Throws { Read-UpdaterSecretKey $openssl $passwordKey $wrong } 'the updater key does not open with the password given'
        }
        Assert-Throws { Read-UpdaterSecretKey $openssl (Get-Content -LiteralPath $fixtureKey -Raw) 'a password' } 'the updater key does not open with the password given'
        # Not a key, or one whose public half is not its own: fixed words,
        # never the text given.
        foreach ($bad in 'not a key', $fixturePub, [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes("untrusted comment: x`nAAAA`n"))) {
            try { Read-UpdaterSecretKey $openssl $bad ''; throw 'took it' } catch {
                Assert-Equal "$_" 'the updater key is not a secret key in the form `tauri signer generate` writes'
            }
        }
        # The public key in the file changed, the checksum made to match: the
        # seed's public key is not it.
        $text = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String((Get-Content -LiteralPath $fixtureKey -Raw).Trim())) -split "`n"
        $bin = [Convert]::FromBase64String($text[1])
        $stream = Get-ScryptStream $openssl ([byte[]]$bin[6..37]) (Get-MinisignScryptParams 1048576 33554432) ''
        $plain = [byte[]]::new(104)
        for ($i = 0; $i -lt 104; $i++) { $plain[$i] = $bin[54 + $i] -bxor $stream[$i] }
        $plain[40] = $plain[40] -bxor 1
        $sum = Get-Blake2b ([byte[]]([byte[]](0x45, 0x64) + $plain[0..71])) 32
        [Array]::Copy($sum, 0, $plain, 72, 32)
        for ($i = 0; $i -lt 104; $i++) { $bin[54 + $i] = $plain[$i] -bxor $stream[$i] }
        $damaged = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes("$($text[0])`n$([Convert]::ToBase64String($bin))`n"))
        Assert-Throws { Read-UpdaterSecretKey $openssl $damaged '' } 'the public key it carries is not its own'
    }

    Check 'scrypt''s parameters are libsodium''s and minisign''s, and a key past what Tauri''s make is refused, saying so' {
        $params = Get-MinisignScryptParams 1048576 33554432
        Assert-Equal "$($params.LogN) $($params.R) $($params.P)" '15 8 1'
        # opslimit below memlimit / 32: N from opslimit, p 1 (libsodium's pickparams).
        $params = Get-MinisignScryptParams 65536 33554432
        Assert-Equal "$($params.LogN) $($params.R) $($params.P)" '11 8 1'
        # Above it: N from memlimit (1 MiB, N = 2^10), p from what opslimit leaves.
        $params = Get-MinisignScryptParams (512 * 1024 * 4) 1048576
        Assert-Equal "$($params.LogN) $($params.R) $($params.P)" '10 8 64'
        $refused = 'not ones the release scripts open: they take memlimit up to 1 GiB, N up to 2^20 and p up to 64'
        # p = 65: scrypt and minisign would take it; refused on purpose.
        Assert-Throws { Get-MinisignScryptParams (520 * 1024 * 4) 1048576 } $refused
        # (opslimit / 4) / N just past 2^32: minisign, narrowing it to 32 bits
        # first, would take p = 1, libsodium, capping it first, 2^30 - 1 over
        # 8; refused rather than opened either way.
        Assert-Throws { Get-MinisignScryptParams ([uint64]4294967304 * 4096) 1048576 } $refused
        Assert-Throws { Get-MinisignScryptParams 1048576 2147483648 } $refused
        Assert-Throws { Get-MinisignScryptParams ([uint64]::MaxValue) 1073741824 } $refused
    }

    Check 'neither the key nor its password is ever on OpenSSL''s command line, and the password only on its input' {
        $real = ${function:Invoke-OpenSsl}
        $runs = [System.Collections.Generic.List[object]]::new()
        # Shadows the function for what this test calls: PowerShell finds a
        # command in the caller's scopes first.
        function Invoke-OpenSsl {
            param([string]$OpenSsl, [string]$What, [string[]]$Arguments, [byte[]]$Stdin = @(), [hashtable]$Environment = @{})
            $runs.Add([pscustomobject]@{ Arguments = $Arguments -join ' '; Environment = $Environment.Clone(); Stdin = [byte[]]$Stdin.Clone() })
            & $real @PSBoundParameters
        }
        $key = Read-UpdaterSecretKey $openssl $passwordKey $password
        $path = & $setupCopy 'argv'
        New-UpdaterSignature -OpenSsl $openssl -Key $key -Path $path -Version '0.20.0' -Fingerprint '0123456789abcdef' -Work (Split-Path $path) -Now $signedAt | Out-Null
        $secrets = @($password, [Convert]::ToHexString($key.Seed), [Convert]::ToBase64String($key.Seed), $passwordKey.Substring(120, 40))
        $secrets += @($runs | ForEach-Object { $_.Environment.Values })
        if ($runs.Count -lt 4) { throw "only $($runs.Count) OpenSSL runs seen" }
        foreach ($run in $runs) {
            foreach ($secret in $secrets) {
                if ($run.Arguments.Contains($secret, [StringComparison]::OrdinalIgnoreCase)) { throw "a secret is on the command line: $($run.Arguments)" }
            }
        }
        $line = [Convert]::ToHexString([Text.Encoding]::UTF8.GetBytes("$password`n"))
        $passed = @($runs | Where-Object { [Convert]::ToHexString($_.Stdin) -ceq $line })
        Assert-Equal $passed.Count 1
        if ($passed[0].Arguments -notmatch '-pkeyopt_passin pass:stdin$') { throw 'the password is not taken from OpenSSL''s input' }
        foreach ($run in $runs) {
            if (@($run.Environment.Values) -contains $password) { throw 'the password is in an OpenSSL run''s environment' }
        }
        Assert-Equal @(Get-ChildItem -LiteralPath (Split-Path $path) -Filter '*.pem').Count 0
    }

    Check 'the release''s signing makes the fixture''s signature again, byte for byte, the same every time' {
        $key = Read-UpdaterSecretKey $openssl (Get-Content -LiteralPath $fixtureKey -Raw) ''
        $path = & $setupCopy 'again'
        $sign = { New-UpdaterSignature -OpenSsl $openssl -Key $key -Path $path -Version '0.20.0' -Fingerprint '0123456789abcdef' -Work (Split-Path $path) -Now $signedAt }
        $made = & $sign
        Assert-Equal $made.Signature $openSslSig
        Assert-Equal $made.Timestamp 1795158621
        Assert-Equal (& $sign).Signature $openSslSig
        Assert-UpdaterSignatureVerifies $path $made.Signature $fixturePub
        # In the form of the one the real plugin took: the same lines but for
        # the signatures' bytes, both dated alike.
        $lines = { param($sig) [Text.Encoding]::ASCII.GetString([Convert]::FromBase64String($sig)) -split "`n" }
        $ours = & $lines $openSslSig; $took = & $lines $pluginSig
        Assert-Equal $ours.Count $took.Count
        Assert-Equal $ours[0] $took[0]
        Assert-Equal $ours[2] $took[2]
        Assert-Equal ([Convert]::ToHexString([byte[]]([Convert]::FromBase64String($ours[1])[0..9]))) ([Convert]::ToHexString([byte[]]([Convert]::FromBase64String($took[1])[0..9])))
    }

    Check 'a signature RFC 8032 does not make for the key is not used, valid or not' {
        $key = Read-UpdaterSecretKey $openssl (Get-Content -LiteralPath $fixtureKey -Raw) ''
        $hash = Invoke-OpenSsl $openssl 'hashing' @('dgst', '-blake2b512', '-binary', (Join-Path $fixtures 'setup.bin'))
        $sig = { param($s) [byte[]]([Convert]::FromBase64String(([Text.Encoding]::ASCII.GetString([Convert]::FromBase64String($s)) -split "`n")[1])[10..73]) }
        Assert-Rfc8032Signature $key.Seed $key.Public $hash (& $sig $openSslSig)
        # minisign's signature with this key over this installer verifies, but
        # its nonce is minisign's own choice: what a signer leaking its key
        # would make looks just like it.
        Assert-UpdaterSignatureVerifies (Join-Path $fixtures 'setup.bin') $pluginSig $fixturePub
        Assert-Throws { Assert-Rfc8032Signature $key.Seed $key.Public $hash (& $sig $pluginSig) } 'not the one RFC 8032 makes'
        $bent = & $sig $openSslSig
        $bent[40] = $bent[40] -bxor 1
        Assert-Throws { Assert-Rfc8032Signature $key.Seed $key.Public $hash $bent } 'not the one RFC 8032 makes'
    }

    Check 'only a signature in the signing''s form, for this installer, version and core, goes out' {
        $pubkey = Get-UpdaterPubkey
        $check = { param($sig, [Nullable[long]]$at) Assert-UpdaterSignatureForm $sig $pubkey 'Luna_0.9.10_x64-setup.exe' '0.9.10' '0123456789abcdef' -Timestamp $at }
        & $check (& $formed $pubkey $ourComment) 1795000000
        & $check (& $formed $pubkey $ourComment) $null
        Assert-Throws { & $check (& $formed $pubkey $ourComment) 1795000001 } 'not dated as it was signed'
        Assert-Throws { & $check "$(& $formed $pubkey $ourComment)`n" $null } 'nothing around it'
        Assert-Throws { & $check " $(& $formed $pubkey $ourComment)" $null } 'nothing around it'
        # Legacy (over the file itself), another key, a short global signature.
        Assert-Throws { & $check (& $formed $pubkey $ourComment 'Ed') $null } 'not a prehashed'
        Assert-Throws { & $check (& $formed $fixturePub $ourComment) $null } 'another key id'
        Assert-Throws { & $check (& $formed $pubkey $ourComment 'ED' 63) $null } 'global signature'
        # A comment for another file, version or core, without the core, with
        # more, or with a timestamp of other digits or more of them.
        $arabicThree = [string][char]0x0663
        foreach ($comment in ($ourComment -replace '0\.9\.10_', '0.9.1_'), ($ourComment -replace 'version:0\.9\.10', 'version:0.9.1'),
            ($ourComment -replace 'abcdef$', 'abcdee'), ($ourComment -replace '\tcore_fingerprint:.*$', ''), "$ourComment`tx:y",
            ($ourComment -replace 'timestamp:\d+', 'timestamp:'), ($ourComment -replace 'timestamp:\d+', 'timestamp:17950000000'),
            ($ourComment -replace 'timestamp:\d+', 'timestamp:+1795000000'), ($ourComment -replace 'timestamp:\d+', 'timestamp: 1795000000')) {
            Assert-Throws { & $check (& $formed $pubkey $comment) $null } 'trusted comment'
        }
        Assert-Throws { & $check (& $formed $pubkey ($ourComment -replace 'timestamp:17', "timestamp:1$arabicThree")) $null } 'printable ASCII'
        # A date far ahead of this clock, without -Timestamp.
        Assert-Throws { & $check (& $formed $pubkey ($ourComment -replace 'timestamp:\d+', 'timestamp:9999999999')) $null } 'ahead of this clock'
        # More lines, or lines ended otherwise than minisign ends them.
        $text = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String((& $formed $pubkey $ourComment)))
        $again = { param($t) [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($t)) }
        Assert-Throws { & $check (& $again "$text`n") $null } 'does not have the 4 lines'
        Assert-Throws { & $check (& $again ($text -replace "`n", "`r`n")) $null } 'printable ASCII'
        # Another untrusted comment, refused in fixed words, not its own.
        try { & $check (& $again ($text -replace 'tauri secret key', 'someone')) $null; throw 'took it' } catch {
            Assert-Equal "$_" 'the signature''s untrusted comment is not Tauri''s'
        }
        # What the Tauri CLI made up to 0.19 names no core: not a release of 0.20's signing.
        Assert-Throws { Assert-UpdaterSignatureForm (& $fixture 'setup.bin.sig') (& $fixture 'key.pub') 'setup.bin' '0.20.0' '0123456789abcdef' } 'trusted comment'
        Assert-UpdaterSignatureForm $openSslSig $fixturePub 'Luna_0.20.0_x64-setup.exe' '0.20.0' '0123456789abcdef' -Timestamp 1795158621
        Assert-UpdaterSignatureForm $pluginSig $fixturePub 'Luna_0.20.0_x64-setup.exe' '0.20.0' '0123456789abcdef' -Timestamp 1795158621
    }

    Check 'OpenSSL verifies a signature, and refuses other bytes, another comment or key' {
        $setup = Join-Path $fixtures 'setup.bin'
        Assert-UpdaterSignatureVerifies $setup $openSslSig $fixturePub
        # The Tauri CLI's own signature, by another key, verifies the same way.
        Assert-UpdaterSignatureVerifies $setup (& $fixture 'setup.bin.sig') (& $fixture 'key.pub')
        $dir = New-Item -ItemType Directory -Force (Join-Path $work 'openssl')
        $other = Join-Path $dir 'other.bin'
        $bytes = [IO.File]::ReadAllBytes($setup); $bytes[5] = $bytes[5] -bxor 1
        [IO.File]::WriteAllBytes($other, $bytes)
        Assert-Throws { Assert-UpdaterSignatureVerifies $other $openSslSig $fixturePub } 'does not verify over the installer'
        # The comment changed after signing, the signatures left as they were.
        $text = [Text.Encoding]::ASCII.GetString([Convert]::FromBase64String($openSslSig))
        $edited = [Convert]::ToBase64String([Text.Encoding]::ASCII.GetBytes(($text -replace '0123456789abcdef', 'fedcba9876543210')))
        Assert-Throws { Assert-UpdaterSignatureVerifies $setup $edited $fixturePub } 'not the one signed'
        # Another key, given the signature's key id so only the cryptography tells.
        Assert-Throws { Assert-UpdaterSignatureVerifies $setup $openSslSig $passwordPub } 'does not verify over the installer'
    }

    # A tree as the sign job checks it out, its updater.json holding the
    # fixture key's public half (or $Pubkey), and a bundle as the build leaves
    # it, setup.bin its installer.
    $signFixture = {
        param([string]$Name, [string]$Pubkey = $fixturePub)
        $root = Join-Path $work $Name
        New-Item -ItemType Directory -Force (Join-Path $root 'src-tauri') | Out-Null
        $source = Read-UpdaterSource
        [ordered]@{ endpoint = $source.Endpoint; pubkey = $Pubkey } | ConvertTo-Json | Set-Content (Join-Path $root $UpdaterSource)
        $bundle = New-Item -ItemType Directory -Force (Join-Path $root 'bundle')
        Copy-Item -LiteralPath (Join-Path $fixtures 'setup.bin') -Destination (Join-Path $bundle 'Luna_0.20.0_x64-setup.exe')
        New-UpdaterManifest -Version '0.20.0' -CoreFingerprint '0123456789abcdef' -Signature '' -SetupName 'Luna_0.20.0_x64-setup.exe' `
            -Endpoint (Get-UpdaterEndpoint $root) | Set-Content (Join-Path $bundle 'latest.json') -Encoding utf8
        [pscustomobject]@{ Root = $root; Bundle = $bundle.FullName }
    }
    $sign = {
        param($f, [hashtable]$More = @{})
        $params = @{ Dir = $f.Bundle; Version = '0.20.0'; Root = $f.Root; Now = $signedAt }
        foreach ($name in $More.Keys) { $params[$name] = $More[$name] }
        Invoke-ReleaseSign @params
    }
    $sigOf = { param($f) Get-Content -LiteralPath (Join-Path $f.Bundle 'Luna_0.20.0_x64-setup.exe.sig') -Raw }
    $signingWork = { param($f) @(Get-ChildItem -LiteralPath (Join-Path $f.Root 'src-tauri/target/release-signing') -Recurse -Force -ErrorAction SilentlyContinue) }
    $unsigned = {
        param($f)
        Assert-Equal (Test-Path -LiteralPath (Join-Path $f.Bundle 'Luna_0.20.0_x64-setup.exe.sig')) $false
        Assert-Equal ((Get-Content -LiteralPath (Join-Path $f.Bundle 'latest.json') -Raw | ConvertFrom-Json).platforms.'windows-x86_64'.signature) ''
        Assert-Equal (& $signingWork $f).Count 0
    }
    # The release environment's secrets, set for $Block alone.
    $withSecrets = {
        param([string]$Key, [string]$Password, [scriptblock]$Block)
        $env:TAURI_SIGNING_PRIVATE_KEY = $Key
        $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = $Password
        try { & $Block } finally {
            Remove-Item Env:TAURI_SIGNING_PRIVATE_KEY, Env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD -ErrorAction SilentlyContinue
        }
    }

    Check 'the release key file signs the installer, OpenSSL checks it, and latest.json takes it' {
        $f = & $signFixture 'sign-file'
        & $sign $f @{ UpdaterKey = $fixtureKey }
        Assert-Equal (& $sigOf $f) $openSslSig
        $manifest = Get-Content -LiteralPath (Join-Path $f.Bundle 'latest.json') -Raw | ConvertFrom-Json
        Assert-Equal $manifest.platforms.'windows-x86_64'.signature $openSslSig
        Assert-Equal $manifest.core_fingerprint '0123456789abcdef'
        Assert-Equal (& $signingWork $f).Count 0
        # The draft check makes the same checks from the published files.
        Assert-ReleaseSignature $f.Bundle '0.20.0' -Root $f.Root
        Assert-Throws { Assert-ReleaseSignature $f.Bundle '0.20.0' -Pubkey (& $fixture 'key.pub') -Root $f.Root } 'another key id'
        Assert-Throws { & $sign $f @{ UpdaterKey = $fixtureKey } } 'is signed already'
    }

    Check 'the release environment''s key signs with its password, and a wrong one signs nothing' {
        $f = & $signFixture 'sign-secret' $passwordPub
        & $withSecrets $passwordKey 'wrong' { Assert-Throws { & $sign $f } 'does not open with the password given' }
        & $unsigned $f
        & $withSecrets $passwordKey $password { & $sign $f }
        Assert-UpdaterSignatureVerifies (Join-Path $f.Bundle 'Luna_0.20.0_x64-setup.exe') (& $sigOf $f) $passwordPub
        Assert-ReleaseSignature $f.Bundle '0.20.0' -Root $f.Root
    }

    Check 'a key installed copies would refuse signs nothing, and a rotation''s bridge is checked against the trusted key' {
        # The release embeds another key; the signature is by the old one.
        $f = & $signFixture 'sign-bridge' (& $fixture 'key.pub')
        Assert-Throws { & $sign $f @{ UpdaterKey = $fixtureKey } } 'is not the one the signature is checked against'
        & $unsigned $f
        & $sign $f @{ UpdaterKey = $fixtureKey; TrustedPubkey = $fixturePub }
        Assert-Equal (& $sigOf $f) $openSslSig
        Assert-ReleaseSignature $f.Bundle '0.20.0' -Pubkey $fixturePub -Root $f.Root
    }

    Check 'a throwaway key is made for the run, signs, and is what the signature is checked against' {
        $f = & $signFixture 'sign-throwaway'
        & $withSecrets 'the real key' '' { & $sign $f @{ Throwaway = $true } }
        $sig = & $sigOf $f
        $lines = [Text.Encoding]::ASCII.GetString([Convert]::FromBase64String($sig)) -split "`n"
        if ([Convert]::ToHexString([byte[]]([Convert]::FromBase64String($lines[1])[2..9])) -ceq [Convert]::ToHexString((Read-UpdaterPubkey $fixturePub).KeyId)) {
            throw 'the throwaway signature is under the release key'
        }
        Assert-Equal $lines[2] "trusted comment: timestamp:1795158621`tfile:Luna_0.20.0_x64-setup.exe`tversion:0.20.0`tcore_fingerprint:0123456789abcdef"
        Assert-Equal (Get-Content -LiteralPath (Join-Path $f.Bundle 'latest.json') -Raw | ConvertFrom-Json).platforms.'windows-x86_64'.signature $sig
        Assert-Equal (& $signingWork $f).Count 0
    }

    Check 'the trusted comment is dated as it is signed, the key open and handed to OpenSSL' {
        $f = & $signFixture 'sign-dated'
        $root = $f.Root
        $asked = [System.Collections.Generic.List[bool]]::new()
        $clock = {
            $asked.Add(@(Get-ChildItem -LiteralPath (Join-Path $root 'src-tauri/target/release-signing') -Recurse -Filter 'signing-key.pem').Count -eq 1)
            [DateTimeOffset]::FromUnixTimeSeconds(1800000000)
        }.GetNewClosure()
        & $sign $f @{ UpdaterKey = $fixtureKey; Now = $clock }
        Assert-Equal ($asked -join ',') 'True'
        $lines = [Text.Encoding]::ASCII.GetString([Convert]::FromBase64String((& $sigOf $f))) -split "`n"
        if (-not $lines[2].StartsWith("trusted comment: timestamp:1800000000`t")) { throw "dated otherwise: $($lines[2])" }
    }

    Check 'a bundle that changes while it is signed is refused' {
        $f = & $signFixture 'sign-changed'
        $bundle = $f.Bundle
        $planting = { Set-Content (Join-Path $bundle 'planted.exe') 'x' -NoNewline; [DateTimeOffset]::FromUnixTimeSeconds(1795158621) }.GetNewClosure()
        Assert-Throws { & $sign $f @{ UpdaterKey = $fixtureKey; Now = $planting } } 'changed while the installer was signed'
        Assert-Equal (Test-Path -LiteralPath (Join-Path $f.Bundle 'Luna_0.20.0_x64-setup.exe.sig')) $false
        Assert-Equal (& $signingWork $f).Count 0
    }

    Check 'nothing is signed without a key or a core fingerprint' {
        $f = & $signFixture 'sign-nothing'
        & $withSecrets '' '' { Assert-Throws { & $sign $f } 'needs the updater''s private key' }
        Assert-Throws { & $sign $f @{ UpdaterKey = (Join-Path $f.Root 'missing.key') } } 'no updater key at'
        & $withSecrets 'not a key' '' { Assert-Throws { & $sign $f } 'not a secret key in the form' }
        $manifest = Join-Path $f.Bundle 'latest.json'
        (Get-Content -LiteralPath $manifest -Raw) -replace '0123456789abcdef', 'NOTAFINGERPRINT0' | Set-Content -LiteralPath $manifest
        Assert-Throws { & $sign $f @{ UpdaterKey = $fixtureKey } } 'not a core fingerprint'
        & $unsigned $f
    }

    Check 'a rotation''s new key is in the Tauri CLI''s form, readable by this user alone, and signs' {
        $path = Join-Path $work 'rotation/updater.key'
        $pubkey = New-UpdaterKey $path
        Assert-Equal (Get-Content -LiteralPath "$path.pub" -Raw) $pubkey
        Resolve-TrustedPubkey $pubkey | Out-Null
        $secret = Get-Content -LiteralPath $path -Raw
        Assert-Equal ([Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($secret)) -split "`n")[0] 'untrusted comment: rsign encrypted secret key'
        if ($IsWindows) {
            # Protected, nothing inherited from the folder: this user, SYSTEM
            # and Administrators alone, as Luna's private files.
            # Windows writes the ACEs back in an order of its own.
            $sddl = [IO.FileSystemAclExtensions]::GetAccessControl([IO.FileInfo]::new($path)).GetSecurityDescriptorSddlForm('Access')
            $aces = { param($d) (@([regex]::Matches($d, '\([^)]*\)') | ForEach-Object Value | Sort-Object) -join '') }
            Assert-Equal (& $aces $sddl) (& $aces (Get-PrivateSddl))
            if ($sddl -cnotmatch '^D:P\(') { throw "the key's DACL is not protected: $sddl" }
        } else {
            Assert-Equal ([IO.File]::GetUnixFileMode($path)) ([IO.UnixFileMode]'UserRead, UserWrite')
        }
        Assert-Throws { New-UpdaterKey $path } 'there already'
        if ($IsWindows) {
            # This folder's volume keeps permissions; one that keeps none
            # (FAT, exFAT) is refused before any key file is there.
            Assert-Equal (Test-PersistentAcls (Split-Path $path)) $true
            function Test-PersistentAcls { $false }
            $fat = Join-Path $work 'rotation-fat/updater.key'
            Assert-Throws { New-UpdaterKey $fat } 'its volume keeps no file permissions'
            Assert-Equal (Test-Path -LiteralPath $fat) $false
            Assert-Equal (Test-Path -LiteralPath "$fat.pub") $false
        }
        $f = & $signFixture 'sign-rotated' $pubkey
        & $sign $f @{ UpdaterKey = $path }
        Assert-ReleaseSignature $f.Bundle '0.20.0' -Root $f.Root
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
        Assert-Throws { Assert-ReleaseWorkflow (& $edit $beforeSign "      - name: Install`n        run: |`n          echo ready`n          pnpm install --frozen-lockfile`n$beforeSign") } 'sign has 7 steps'
        Assert-Throws { Assert-ReleaseWorkflow (& $edit $beforeSign "      - uses: actions/setup-node@0123456789abcdef0123456789abcdef01234567`n$beforeSign") } 'sign has 7 steps'
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

    Check 'release.ps1 -Local hands -TrustedPubkey on to the signing' {
        $ast = [System.Management.Automation.Language.Parser]::ParseFile((Join-Path $RepoRoot 'scripts/release.ps1'), [ref]$null, [ref]$null)
        $param = $ast.ParamBlock.Parameters | Where-Object { $_.Name.VariablePath.UserPath -eq 'TrustedPubkey' }
        if (-not $param) { throw 'release.ps1 has no -TrustedPubkey' }
        Assert-Equal $param.DefaultValue.Extent.Text '$env:LUNA_UPDATER_TRUSTED_PUBKEY'
        $calls = @($ast.FindAll({ param($n) $n -is [System.Management.Automation.Language.CommandAst] -and $n.Extent.Text -match 'release-sign\.ps1' }, $true))
        Assert-Equal $calls.Count 1
        $elements = @($calls[0].CommandElements | ForEach-Object { $_.Extent.Text })
        $at = [array]::IndexOf($elements, '-TrustedPubkey')
        if ($at -lt 0 -or $elements[$at + 1] -cne '$TrustedPubkey') { throw "release-sign.ps1 is called as $($calls[0].Extent.Text)" }
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

    Check 'the scripts load in the sign job''s checkout, which holds only its own files' {
        # The files its sparse checkout takes, as the allowlist above has them.
        $files = @($SignSteps[0].with -split '\r?\n' | Where-Object { $_ -match '^/' } | ForEach-Object { $_.TrimStart('/') })
        if (-not ($files -contains 'scripts/lib.ps1')) { throw 'the allowlist no longer names scripts/lib.ps1' }
        # An apostrophe in the path, as a checkout under O'Brien would have:
        # the path goes to the child as an argument, never into its source.
        $tree = Join-Path $work "sign-checkout-O'Brien"
        foreach ($file in $files) {
            $to = Join-Path $tree $file
            New-Item -ItemType Directory -Force (Split-Path $to) | Out-Null
            Copy-Item -LiteralPath (Join-Path $RepoRoot $file) -Destination $to
        }
        $out = pwsh -NoProfile -Command {
            param($tree)
            . (Join-Path $tree 'scripts/lib.ps1')
            . (Join-Path $tree 'scripts/release-lib.ps1')
            Get-OpenSsl
            Get-UpdaterPubkey
        } -args $tree 2>&1
        if ($LASTEXITCODE) { throw "loading failed: $out" }
        if ("$($out[0])" -notmatch 'openssl') { throw "Get-OpenSsl gave '$($out[0])'" }
        Assert-Equal "$($out[1])" (Get-UpdaterPubkey)
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
