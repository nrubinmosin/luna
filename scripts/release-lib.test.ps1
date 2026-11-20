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
    needs: [verify, build, signer]
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
/docker/linux-check.Dockerfile
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
        uses = 'actions/download-artifact@9000827ccba6bdab643e8b6fd33ac0654aef8333'
        with = @'
name: signer
path: ${{ runner.temp }}/signer
'@
    }
    [ordered]@{
        name = 'Check the bundle and the signer against their checksums'
        env  = @'
SUMS: ${{ needs.build.outputs.sums }}
SIGNER_SUMS: ${{ needs.signer.outputs.sums }}
'@
        run  = @'
|
. ./scripts/lib.ps1
. ./scripts/release-lib.ps1
Assert-AssetChecksums "$env:RUNNER_TEMP/assets" $env:SUMS
Assert-AssetChecksums "$env:RUNNER_TEMP/signer" $env:SIGNER_SUMS
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
        run  = './scripts/release-sign.ps1 -Dir "$env:RUNNER_TEMP/assets" -Version $env:VERSION -Signer "$env:RUNNER_TEMP/signer/luna-updater-signer" -RequireTrustedPubkey'
    }
    [ordered]@{
        name = 'Sign a copy with a throwaway key'
        if   = 'needs.verify.outputs.signed != ''true'''
        env  = 'VERSION: ${{ needs.verify.outputs.version }}'
        run  = @'
|
Copy-Item -Recurse "$env:RUNNER_TEMP/assets" "$env:RUNNER_TEMP/throwaway"
./scripts/release-sign.ps1 -Dir "$env:RUNNER_TEMP/throwaway" -Version $env:VERSION -Signer "$env:RUNNER_TEMP/signer/luna-updater-signer" -Throwaway
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

    Check 'Cargo.toml and Cargo.lock are all a tree needs, and it needs both' {
        $root = New-Tree cargo-only
        foreach ($file in $ReleaseVersionOptional) { Remove-Item -LiteralPath (Join-Path $root $file) -ErrorAction SilentlyContinue }
        Assert-Equal ((Get-ReleaseVersionFiles $root) -join ',') 'src-tauri/Cargo.toml,src-tauri/Cargo.lock'
        Set-ReleaseVersion '7.8.9' $root
        Assert-Equal (Get-ReleaseVersion $root) '7.8.9'
        foreach ($file in $ReleaseVersionOptional) { Assert-Equal (Test-Path -LiteralPath (Join-Path $root $file)) $false }
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
        Set-Content (Join-Path $nsis 'Luna_0.9.10_x64-setup.exe') 'x'
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
        if ($updater -match 'tauri\.conf\.json"\)') { throw 'updater.rs reads tauri.conf.json' }
        # While tauri.conf.json is there, its plugin config says the same.
        $conf = Join-Path $RepoRoot 'src-tauri/tauri.conf.json'
        if (Test-Path -LiteralPath $conf) {
            $plugin = (Get-Content -LiteralPath $conf -Raw | ConvertFrom-Json).plugins.updater
            if ($plugin) {
                Assert-Equal $plugin.pubkey $source.Pubkey
                Assert-Equal (@($plugin.endpoints) -join ' ') $source.Endpoint
            }
        }
        $root = New-Item -ItemType Directory -Force (Join-Path $work 'source/src-tauri')
        Set-Content (Join-Path $root 'updater.json') '{"endpoint":"https://github.com/a/b/releases/latest/download/latest.json","pubkey":"x","extra":1}'
        Assert-Throws { Read-UpdaterSource (Split-Path $root) } 'endpoint and pubkey, and nothing else'
        Set-Content (Join-Path $root 'updater.json') '{"endpoint":"","pubkey":"x"}'
        Assert-Throws { Read-UpdaterSource (Split-Path $root) } 'has no endpoint'
    }

    # A signature in the signer's form, its signatures zeros: the form check
    # reads the lines, never the cryptography (the signer's `verify` does).
    $formed = {
        param([string]$Pubkey, [string]$Comment, [string]$Algorithm = 'ED', [int]$Global = 64)
        $key = [Convert]::FromBase64String(([Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($Pubkey)) -split '\r?\n')[1])
        $bin = [byte[]]([Text.Encoding]::ASCII.GetBytes($Algorithm) + $key[2..9] + [byte[]]::new(64))
        $file = "untrusted comment: signature from tauri secret key`n$([Convert]::ToBase64String($bin))`ntrusted comment: $Comment`n$([Convert]::ToBase64String([byte[]]::new($Global)))`n"
        [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($file))
    }
    $ourComment = "timestamp:1795000000`tfile:Luna_0.9.10_x64-setup.exe`tversion:0.9.10`tcore_fingerprint:0123456789abcdef"
    $fixtures = Join-Path $RepoRoot 'src-tauri/fixtures/updater'

    Check 'only a signature in the signer''s form, for this installer, version and core, goes out' {
        $pubkey = Get-UpdaterPubkey
        $check = { param($sig) Assert-UpdaterSignatureForm $sig $pubkey 'Luna_0.9.10_x64-setup.exe' '0.9.10' '0123456789abcdef' }
        & $check (& $formed $pubkey $ourComment)
        Assert-Throws { & $check "$(& $formed $pubkey $ourComment)`n" } 'nothing around it'
        Assert-Throws { & $check " $(& $formed $pubkey $ourComment)" } 'nothing around it'
        # Legacy (over the file itself), another key, a short global signature.
        Assert-Throws { & $check (& $formed $pubkey $ourComment 'Ed') } 'not a prehashed'
        Assert-Throws { & $check (& $formed (Get-Content -LiteralPath (Join-Path $fixtures 'tauri.key.pub') -Raw) $ourComment) } 'another key id'
        Assert-Throws { & $check (& $formed $pubkey $ourComment 'ED' 63) } 'global signature'
        # A comment for another file, version or core, without the core, or with more.
        foreach ($comment in ($ourComment -replace '0\.9\.10_', '0.9.1_'), ($ourComment -replace 'version:0\.9\.10', 'version:0.9.1'),
            ($ourComment -replace 'abcdef$', 'abcdee'), ($ourComment -replace '\tcore_fingerprint:.*$', ''), "$ourComment`tx:y",
            ($ourComment -replace 'timestamp:\d+', 'timestamp:')) {
            Assert-Throws { & $check (& $formed $pubkey $comment) } 'trusted comment'
        }
        # More lines, or lines ended otherwise than minisign ends them.
        $text = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String((& $formed $pubkey $ourComment)))
        $again = { param($t) [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($t)) }
        Assert-Throws { & $check (& $again "$text`n") } '5 lines'
        Assert-Throws { & $check (& $again ($text -replace "`n", "`r`n")) } 'end its lines'
        Assert-Throws { & $check (& $again ($text -replace 'tauri secret key', 'someone')) } 'untrusted comment'
        # What the Tauri CLI made up to 0.19 names no core: not a release of 0.20's signer.
        $tauri = (Get-Content -LiteralPath (Join-Path $fixtures 'setup.bin.sig') -Raw).Trim()
        Assert-Throws { Assert-UpdaterSignatureForm $tauri (Get-Content -LiteralPath (Join-Path $fixtures 'key.pub') -Raw) 'setup.bin' '0.20.0' '0123456789abcdef' } 'trusted comment'
    }

    # A tree as the sign job checks it out (the updater's endpoint and key),
    # a bundle as the build leaves it, and a stand-in for the signer's binary
    # as the signer job builds it. Docker is stood in for by $Run
    # (Invoke-ReleaseSign).
    $signFixture = {
        param([string]$Name)
        $root = Join-Path $work $Name
        New-Item -ItemType Directory -Force (Join-Path $root 'src-tauri') | Out-Null
        Copy-Item -LiteralPath (Join-Path $RepoRoot $UpdaterSource) -Destination (Join-Path $root 'src-tauri')
        $bundle = New-Item -ItemType Directory -Force (Join-Path $root 'bundle')
        Set-Content (Join-Path $bundle 'Luna_0.9.10_x64-setup.exe') 'setup' -NoNewline
        New-UpdaterManifest -Version '0.9.10' -CoreFingerprint '0123456789abcdef' -Signature '' -SetupName 'Luna_0.9.10_x64-setup.exe' `
            -Endpoint (Get-UpdaterEndpoint $root) | Set-Content (Join-Path $bundle 'latest.json') -Encoding utf8
        $key = Join-Path $root 'updater.key'
        Set-Content $key 'not a key' -NoNewline
        $signer = Join-Path $root 'luna-updater-signer'
        Set-Content $signer 'the signer' -NoNewline
        [pscustomobject]@{
            Root = $root; Bundle = $bundle.FullName; Key = $key; Signer = $signer
            SignAs = Get-UpdaterPubkey $root; Sig = $null; Calls = [System.Collections.Generic.List[object]]::new()
        }
    }
    # The stand-in for docker: records each run, checks the signer mounted is
    # the one given, and does what each step's signer would: a key pair, a
    # .sig in the signer's form under $f.SignAs (or $f.Sig as it is), a check
    # that passes unless told otherwise.
    $fakeRun = {
        param($f, [bool]$FailCheck = $false)
        # Copies here, for the closure: it sees this scope, not the script's.
        $formed = $formed; $ourComment = $ourComment; $fixtures = $fixtures
        {
            param($What, $DockerArgs)
            $f.Calls.Add([pscustomobject]@{ What = $What; Args = @($DockerArgs) })
            $mount = @($DockerArgs | Where-Object { $_ -like '*:/signer:ro' }) -replace ':/signer:ro$', ''
            if ((Get-Content -LiteralPath (Join-Path $mount 'luna-updater-signer') -Raw) -cne 'the signer') { throw 'not the signer given' }
            if ($What -eq 'making a throwaway key') {
                $keys = ($DockerArgs | Where-Object { $_ -like '*:/keys' }) -replace ':/keys$', ''
                Set-Content (Join-Path $keys 'updater.key') 'a throwaway key' -NoNewline
                Set-Content (Join-Path $keys 'updater.key.pub') (Get-Content -LiteralPath (Join-Path $fixtures 'tauri-password.key.pub') -Raw) -NoNewline
            } elseif ($What -eq 'signing') {
                $sig = if ($null -ne $f.Sig) { $f.Sig } else { & $formed $f.SignAs $ourComment }
                Set-Content (Join-Path $f.Bundle 'Luna_0.9.10_x64-setup.exe.sig') $sig -NoNewline
            } elseif ($FailCheck) { throw "$What failed (exit 1)" }
        }.GetNewClosure()
    }

    Check 'signing runs the given signer isolated, then checks against the updater''s key' {
        $f = & $signFixture 'sign-ok'
        Invoke-ReleaseSign -Dir $f.Bundle -Version '0.9.10' -UpdaterKey $f.Key -Signer $f.Signer -Root $f.Root -Run (& $fakeRun $f)
        Assert-Equal (@($f.Calls.What) -join ',') 'signing,the signature check'
        foreach ($call in $f.Calls) {
            $text = $call.Args -join ' '
            foreach ($flag in '--rm', '--network none', '--read-only', '--cap-drop ALL', '--security-opt no-new-privileges') {
                if (-not $text.Contains($flag)) { throw "$($call.What) runs without $flag" }
            }
            Assert-Equal $call.Args[[array]::IndexOf($call.Args, (Get-SignerImage)) + 1] '/signer/luna-updater-signer'
        }
        foreach ($call in $f.Calls) {
            $user = [array]::IndexOf($call.Args, '--user')
            if ($IsWindows) { Assert-Equal $user -1 }
            else { Assert-Equal $call.Args[$user + 1] "$(id -u):$(id -g)" }
        }
        $sign = $f.Calls[0].Args
        if (-not ($sign -join ' ').Contains('TAURI_SIGNING_PRIVATE_KEY_PATH=/keys/updater.key') -or -not ($sign -join ' ').Contains(':/keys/updater.key:ro')) { throw 'the key is not mounted for signing' }
        Assert-Equal (@($sign[([array]::IndexOf($sign, 'sign'))..($sign.Count - 1)]) -join ' ') 'sign /out/Luna_0.9.10_x64-setup.exe 0.9.10 0123456789abcdef'
        $check = $f.Calls[1].Args
        if (($check -join ' ') -match 'keys|TAURI_SIGNING') { throw 'the check is handed the key' }
        if (-not ($check -join ' ').Contains(':/out:ro')) { throw 'the check can write the bundle' }
        Assert-Equal (@($check[([array]::IndexOf($check, 'verify'))..($check.Count - 1)]) -join ' ') `
            "verify /out/Luna_0.9.10_x64-setup.exe /out/Luna_0.9.10_x64-setup.exe.sig $(Get-UpdaterPubkey $f.Root) 0.9.10 0123456789abcdef"
        $manifest = Get-Content -LiteralPath (Join-Path $f.Bundle 'latest.json') -Raw | ConvertFrom-Json
        Assert-Equal $manifest.platforms.'windows-x86_64'.signature (& $formed (Get-UpdaterPubkey) $ourComment)
        Assert-Equal $manifest.core_fingerprint '0123456789abcdef'
        Assert-Equal (@(Get-ChildItem -LiteralPath (Join-Path $f.Root 'src-tauri/target/release-signer')).Count) 0
    }

    Check 'a rotation''s bridge release is checked against the trusted key, not the one it carries' {
        $f = & $signFixture 'sign-bridge'
        $old = (Get-Content -LiteralPath (Join-Path $fixtures 'tauri.key.pub') -Raw).Trim()
        $f.SignAs = $old
        Invoke-ReleaseSign -Dir $f.Bundle -Version '0.9.10' -UpdaterKey $f.Key -Signer $f.Signer -Root $f.Root -TrustedPubkey $old -Run (& $fakeRun $f)
        $check = $f.Calls[1].Args
        Assert-Equal $check[[array]::IndexOf($check, 'verify') + 3] $old
    }

    Check 'a throwaway key is made in the signer''s container, signs, and is what the signature is checked against' {
        $f = & $signFixture 'sign-throwaway'
        $f.SignAs = Get-Content -LiteralPath (Join-Path $fixtures 'tauri-password.key.pub') -Raw
        $env:TAURI_SIGNING_PRIVATE_KEY = 'the real key'
        try {
            Invoke-ReleaseSign -Dir $f.Bundle -Version '0.9.10' -Throwaway -Signer $f.Signer -Root $f.Root -Run (& $fakeRun $f)
        } finally { Remove-Item Env:TAURI_SIGNING_PRIVATE_KEY }
        Assert-Equal (@($f.Calls.What) -join ',') 'making a throwaway key,signing,the signature check'
        $generate = $f.Calls[0].Args -join ' '
        foreach ($flag in '--network none', '--read-only', '--cap-drop ALL') {
            if (-not $generate.Contains($flag)) { throw "making the key runs without $flag" }
        }
        $sign = $f.Calls[1].Args
        if (($sign -join ' ') -match 'TAURI_SIGNING_PRIVATE_KEY(?!_PATH)') { throw 'the real key reaches a throwaway signing' }
        if (-not (($sign -join ' ').Contains('updater.key:/keys/updater.key:ro'))) { throw 'the throwaway key is not mounted for signing' }
        $check = $f.Calls[2].Args
        Assert-Equal $check[[array]::IndexOf($check, 'verify') + 3] $f.SignAs.Trim()
        Assert-Equal (@(Get-ChildItem -LiteralPath (Join-Path $f.Root 'src-tauri/target/release-signer')).Count) 0
    }

    Check 'a signature that fails the check, or is not in the signer''s form, is taken away again' {
        $f = & $signFixture 'sign-fail'
        Assert-Throws {
            Invoke-ReleaseSign -Dir $f.Bundle -Version '0.9.10' -UpdaterKey $f.Key -Signer $f.Signer -Root $f.Root -Run (& $fakeRun $f $true)
        } 'the signature check failed'
        Assert-Equal (Test-Path -LiteralPath (Join-Path $f.Bundle 'Luna_0.9.10_x64-setup.exe.sig')) $false
        Assert-Equal ((Get-Content -LiteralPath (Join-Path $f.Bundle 'latest.json') -Raw | ConvertFrom-Json).platforms.'windows-x86_64'.signature) ''
        # The check passed, but what the signer wrote carries more than a signature.
        $f = & $signFixture 'sign-unformed'
        $f.Sig = "$(& $formed $f.SignAs $ourComment)`n"
        Assert-Throws {
            Invoke-ReleaseSign -Dir $f.Bundle -Version '0.9.10' -UpdaterKey $f.Key -Signer $f.Signer -Root $f.Root -Run (& $fakeRun $f)
        } 'nothing around it'
        Assert-Equal (Test-Path -LiteralPath (Join-Path $f.Bundle 'Luna_0.9.10_x64-setup.exe.sig')) $false
        Assert-Equal ((Get-Content -LiteralPath (Join-Path $f.Bundle 'latest.json') -Raw | ConvertFrom-Json).platforms.'windows-x86_64'.signature) ''
    }

    Check 'nothing is signed without a signer, a core fingerprint or a key' {
        $f = & $signFixture 'sign-nothing'
        Assert-Throws {
            Invoke-ReleaseSign -Dir $f.Bundle -Version '0.9.10' -UpdaterKey $f.Key -Signer (Join-Path $f.Root 'missing') -Root $f.Root -Run (& $fakeRun $f)
        } 'no signer binary'
        Assert-Throws { Invoke-ReleaseSign -Dir $f.Bundle -Version '0.9.10' -Signer $f.Signer -Root $f.Root -Run (& $fakeRun $f) } 'needs the updater''s private key'
        $manifest = Join-Path $f.Bundle 'latest.json'
        (Get-Content -LiteralPath $manifest -Raw) -replace '0123456789abcdef', 'NOTAFINGERPRINT0' | Set-Content -LiteralPath $manifest
        Assert-Throws {
            Invoke-ReleaseSign -Dir $f.Bundle -Version '0.9.10' -UpdaterKey $f.Key -Signer $f.Signer -Root $f.Root -Run (& $fakeRun $f)
        } 'not a core fingerprint'
        Assert-Equal $f.Calls.Count 0
        Assert-Throws { Build-UpdaterSigner $f.Bundle } 'there already'
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
        Assert-Equal ($workflow.Jobs.Keys -join ',') 'verify,build,signer,sign,sbom,attest,publish'
        Assert-ReleaseWorkflow $text

        # Each breach, as an edit of the real file.
        $edit = { param($from, $to) if (-not $text.Contains($from)) { throw "no '$from' in release.yml" }; $text.Replace($from, $to) }
        $beforeSign = '      # The key goes to this step alone'
        $signRun = '        run: ./scripts/release-sign.ps1 -Dir "$env:RUNNER_TEMP/assets" -Version $env:VERSION -Signer "$env:RUNNER_TEMP/signer/luna-updater-signer" -RequireTrustedPubkey'
        Assert-Throws { Assert-ReleaseWorkflow (& $edit $beforeSign "      - name: Install`n        run: |`n          echo ready`n          pnpm install --frozen-lockfile`n$beforeSign") } 'sign has 8 steps'
        Assert-Throws { Assert-ReleaseWorkflow (& $edit $beforeSign "      - uses: actions/setup-node@0123456789abcdef0123456789abcdef01234567`n$beforeSign") } 'sign has 8 steps'
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
            Get-SignerImage
            Get-UpdaterPubkey
        } -args $tree 2>&1
        if ($LASTEXITCODE) { throw "loading failed: $out" }
        if ("$($out[0])" -notmatch '^rust:[^@]+@sha256:') { throw "Get-SignerImage gave '$($out[0])'" }
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
