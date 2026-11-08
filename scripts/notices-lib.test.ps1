#Requires -Version 7.4
# Tests for notices-lib.ps1, on a made-up crate tree and license folders of
# their own under test-build/.
# Run: pwsh scripts/notices-lib.test.ps1 (the gate runs it with the frontend group)
. "$PSScriptRoot/lib.ps1"
. "$PSScriptRoot/notices-lib.ps1"

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

$work = Join-Path $RepoRoot "test-build/notices-lib-tests/$([guid]::NewGuid().ToString('n'))"

# cargo metadata's shape, cut down: the root depends on `linked` normally, on
# `generator` as a build dependency, on `derive` (a proc macro) normally and on
# `fixture` for tests; `linked` depends on `deep`, `derive` on `quoting`, and
# `generator` on `deep` as well.
function New-Metadata {
    $package = { param($name, $kind = 'lib') [pscustomobject]@{ id = "$name 1.0.0"; name = $name; version = '1.0.0'; license = 'MIT'; targets = @([pscustomobject]@{ kind = @($kind) }) } }
    $edge = { param($to, $kind) [pscustomobject]@{ pkg = "$to 1.0.0"; dep_kinds = @([pscustomobject]@{ kind = $kind; target = $null }) } }
    [pscustomobject]@{
        packages = @(
            (& $package root bin), (& $package linked), (& $package deep), (& $package generator),
            (& $package derive proc-macro), (& $package quoting), (& $package fixture)
        )
        resolve  = [pscustomobject]@{
            root  = 'root 1.0.0'
            nodes = @(
                [pscustomobject]@{ id = 'root 1.0.0'; deps = @((& $edge linked $null), (& $edge generator 'build'), (& $edge derive $null), (& $edge fixture 'dev')) }
                [pscustomobject]@{ id = 'linked 1.0.0'; deps = @(& $edge deep $null) }
                [pscustomobject]@{ id = 'generator 1.0.0'; deps = @(& $edge deep $null) }
                [pscustomobject]@{ id = 'derive 1.0.0'; deps = @(& $edge quoting $null) }
                [pscustomobject]@{ id = 'deep 1.0.0'; deps = @() }
                [pscustomobject]@{ id = 'quoting 1.0.0'; deps = @() }
                [pscustomobject]@{ id = 'fixture 1.0.0'; deps = @() }
            )
        }
    }
}

# A component for Add-LicenseTexts, with the given files in its staged and
# supplied folders.
function New-Component([string]$Name, [hashtable]$Staged = @{}, [hashtable]$Supplied = @{}) {
    $folders = foreach ($pair in @(@('staged', $Staged), @('supplied', $Supplied))) {
        $dir = Join-Path $work "$($pair[0])/$Name"
        foreach ($file in $pair[1].Keys) {
            $path = Join-Path $dir $file
            New-Item -ItemType Directory -Force (Split-Path $path) | Out-Null
            Set-Content -LiteralPath $path $pair[1][$file] -NoNewline
        }
        $dir
    }
    [pscustomobject]@{ Name = $Name; Version = '1.0.0'; License = 'MIT'; Source = "https://crates.io/crates/$Name/1.0.0"; Note = $null; Staged = $folders[0]; Supplied = $folders[1] }
}

try {
    Check 'only normal dependencies outside proc macros ship' {
        Assert-Equal ((Get-ShippedCrates (New-Metadata)).name -join ' ') 'deep linked'
    }

    Check 'a crate the root reaches only through a build dependency does not ship' {
        $metadata = New-Metadata
        ($metadata.resolve.nodes | Where-Object id -eq 'linked 1.0.0').deps = @()
        Assert-Equal ((Get-ShippedCrates $metadata).name -join ' ') 'linked'
    }

    Check 'a shipped crate without a license text fails, naming it' {
        $components = @(
            (New-Component with -Staged @{ LICENSE = 'MIT License, Copyright the authors' })
            (New-Component without)
        )
        Assert-Throws { Add-LicenseTexts $components } 'no license text for without 1.0.0'
    }

    Check 'an SPDX identifier file or an empty file is no license text' {
        $components = @(New-Component idonly -Staged @{ 'LICENSE.spdx' = 'SPDX-License-Identifier: MIT'; LICENSE = "  `n" })
        Assert-Throws { Add-LicenseTexts $components } 'no license text for idonly'
    }

    Check 'a text kept in the repo stands in for one the package lacks, never over its own' {
        $done = Add-LicenseTexts @(
            (New-Component kept -Supplied @{ 'LICENSE-MIT' = 'kept text' })
            (New-Component own -Staged @{ LICENSE = 'own text' } -Supplied @{ LICENSE = 'kept text' })
        )
        Assert-Equal $done[0].Texts[0].Text 'kept text'
        Assert-Equal $done[1].Texts[0].Text 'own text'
    }

    Check 'texts are read in path order with uniform line ends, nested ones by their path' {
        $done = Add-LicenseTexts @(New-Component nested -Staged @{ 'LICENSE-MIT' = "b`r`nline  `r`n"; 'licenses/Apache-2.0.txt' = 'a' })
        Assert-Equal ($done[0].Texts.Name -join ' ') 'LICENSE-MIT licenses/Apache-2.0.txt'
        Assert-Equal $done[0].Texts[0].Text "b`nline"
    }

    Check 'the notices give each entry, write a shared text once, and come out the same twice' {
        $items = Add-LicenseTexts @(
            (New-Component alpha -Staged @{ LICENSE = 'shared words' })
            (New-Component beta -Staged @{ LICENSE = 'shared words'; NOTICE = 'beta notice' })
        )
        $items[1].License = 'MPL-2.0'
        $items[1].Note = Get-LicenseNote 'MPL-2.0'
        $sections = @([pscustomobject]@{ Title = 'Crates'; Items = $items })
        $text = New-NoticesText '1.2.3' $sections
        if ($text -notmatch 'Third-party notices for Luna 1\.2\.3') { throw 'no header' }
        if ($text -notmatch "(?m)^beta 1\.0\.0`nLicense: MPL-2\.0`nSource: https://crates\.io/crates/beta/1\.0\.0`nThis is Covered Software") { throw "no MPL source statement: $text" }
        Assert-Equal ([regex]::Matches($text, 'shared words').Count) 1
        Assert-Equal ([regex]::Matches($text, 'LICENSE: shared text 1').Count) 2
        if ($text -notmatch "--- NOTICE`nbeta notice") { throw 'no NOTICE text' }
        Assert-Equal (New-NoticesText '1.2.3' $sections) $text
    }

    Check 'only MPL-2.0 code gets the source statement' {
        Assert-Equal (Get-LicenseNote 'MIT OR Apache-2.0') $null
        if (-not (Get-LicenseNote 'MIT OR MPL-2.0')) { throw 'no statement for MPL-2.0 as an alternative' }
    }
} finally {
    Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
}

if ($failures) {
    Write-Host "$failures test(s) failed" -ForegroundColor Red
    exit 1
}
Write-Host 'notices-lib tests passed' -ForegroundColor Green
