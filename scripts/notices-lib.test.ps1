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

    Check 'the standard library''s inventory reads as text, license texts keeping their lines' {
        $html = @"
<html><head><title>Copyright notices</title><style>p { x }</style></head><body>
<h1>Copyright notices for The Rust Standard Library</h1>
<p><b>License:</b> Apache-2.0 OR MIT</p>
<p><b>Copyright:</b> The Rust Project Developers</p>
<h2 id="out-of-tree-dependencies">Out-of-tree dependencies</h2>
<h3>&#128230; aho-corasick-1.1.4</h3>
<p><b>Authors:</b> Andrew Gallant &#60;jamslam@gmail.com&#62;</p>
<details><summary><code>LICENSE-MIT</code></summary><pre>
The MIT License (MIT)

    Copyright (c) 2015 Andrew Gallant &amp; &#34;others&#34;
</pre></details>
</body></html>
"@
        $text = ConvertFrom-RustCopyright $html
        if ($text -match '</?(b|p|h\d|pre|code|summary|details|html|head|body|style|title)\b|&#|&amp;|\{ x \}|Copyright notices</') { throw "tags, entities or head left: $text" }
        if ($text -match '(?m)^Copyright notices$') { throw "the head's title is left: $text" }
        if ($text -notmatch "(?m)^License: Apache-2\.0 OR MIT$") { throw "no license line: $text" }
        if ($text -notmatch "Authors: Andrew Gallant <jamslam@gmail\.com>") { throw "entities not decoded: $text" }
        if (-not $text.Contains("--- LICENSE-MIT`nThe MIT License (MIT)`n`n    Copyright (c) 2015 Andrew Gallant & `"others`"")) { throw "the license text lost its lines: $text" }
        Assert-Equal ($text -match "`n`n`n") $false
    }

    Check 'the in-tree license ids are read from before the out-of-tree crates only' {
        $html = '<p><b>License:</b> Apache-2.0 OR MIT</p><p><b>License:</b> BSD-2-Clause AND (Apache-2.0 OR MIT)</p><p><b>License:</b> Unicode-3.0</p>' +
            '<h2 id="out-of-tree-dependencies">x</h2><p><b>License:</b> MPL-2.0</p>'
        Assert-Equal ((Get-RustInTreeLicenses $html) -join ' ') 'Apache-2.0 BSD-2-Clause MIT Unicode-3.0'
    }

    # A built interface of the given text, as dist/ would hold it.
    $newDist = {
        param($name, $js)
        $dir = Join-Path $work "dist-$name"
        New-Item -ItemType Directory -Force (Join-Path $dir 'assets') | Out-Null
        Set-Content -LiteralPath (Join-Path $dir 'assets/index.js') $js -NoNewline
        Set-Content -LiteralPath (Join-Path $dir 'index.html') '<script type="module" src="/assets/index.js"></script>' -NoNewline
        $dir
    }
    $vite = [pscustomobject]@{ name = 'Vite helpers'; markers = @('vite-preload-helper', 'vite-map-deps') }
    $bundle = 'const __vite__mapDeps=(i,m=__vite__mapDeps)=>i;window.dispatchEvent(new Event("vite:preloadError"));'

    Check 'every helper the bundle holds is listed, and every listed one is in it' {
        Assert-BundledHelpers (& $newDist ok $bundle) @($vite)
        $cjs = 'function Mk(i){return i&&i.__esModule&&Object.prototype.hasOwnProperty.call(i,"default")?i.default:i}'
        Assert-Throws { Assert-BundledHelpers (& $newDist unlisted "$bundle$cjs") @($vite) } 'rollup-commonjs-default-export is in the bundle'
        $polyfill = [pscustomobject]@{ name = 'polyfill'; markers = @('vite-modulepreload-polyfill') }
        Assert-Throws { Assert-BundledHelpers (& $newDist stale $bundle) @($vite, $polyfill) } 'vite-modulepreload-polyfill is listed'
        Assert-Throws { Assert-BundledHelpers (& $newDist ok2 $bundle) @([pscustomobject]@{ name = 'x'; markers = @('nonsense') }) } 'does not know: nonsense'
        Assert-Throws { Assert-BundledHelpers (Join-Path $work 'no-dist') @($vite) } 'no built interface'
    }

    Check 'a component checked against another version than the lockfile''s fails' {
        $cargo = "[[package]]`nname = `"webview2-com-sys`"`nversion = `"0.39.1`"`n"
        $pnpm = "packages:`n`n  vite@7.3.7:`n    resolution: {}`n`n  '@tauri-apps/cli@2.12.3':`n    resolution: {}`n"
        $tied = { param($kind, $name, $version) [pscustomobject]@{ name = "c $name"; tiedTo = [pscustomobject]@{ $kind = $name; version = $version } } }
        Assert-ComponentTies @((& $tied crate webview2-com-sys 0.39.1), (& $tied npm vite 7.3.7), (& $tied npm '@tauri-apps/cli' 2.12.3), [pscustomobject]@{ name = 'untied' }) $cargo $pnpm
        Assert-Throws { Assert-ComponentTies @(& $tied crate webview2-com-sys 0.39.0) $cargo $pnpm } 'checked against crate webview2-com-sys 0.39.0, and the lockfile has 0.39.1'
        Assert-Throws { Assert-ComponentTies @(& $tied npm '@tauri-apps/cli' 2.13.0) $cargo $pnpm } 'the lockfile has 2.12.3'
        Assert-Throws { Assert-ComponentTies @(& $tied npm left-pad 1.0.0) $cargo $pnpm } 'the lockfile has none'
    }

    Check 'a component without texts needs its reason, and then passes without them' {
        $base = @{ section = 'installer'; name = 'runtime'; version = '1'; license = 'terms'; source = 'https://example.invalid'; note = 'linked statically.' }
        Assert-Throws { ConvertTo-ComponentItem ([pscustomobject]$base) $work } 'neither its texts nor why'
        $item = ConvertTo-ComponentItem ([pscustomobject]($base + @{ noText = 'terms, not a text.' })) $work
        $done = Add-LicenseTexts @($item)
        Assert-Equal @($done[0].Texts).Count 0
        Assert-Equal $done[0].Note 'linked statically. terms, not a text.'
        $blank = $base.Clone(); $blank.note = ' '; $blank.texts = 'x'
        Assert-Throws { ConvertTo-ComponentItem ([pscustomobject]$blank) $work } "without 'note'"
    }

    Check 'every entry of the repo''s components.json has its texts here' {
        $supplied = Join-Path $RepoRoot 'third-party-licenses'
        $components = @((Get-Content (Join-Path $supplied 'components.json') -Raw | ConvertFrom-Json).components)
        $done = Add-LicenseTexts @($components | ForEach-Object { ConvertTo-ComponentItem $_ $supplied })
        Assert-Equal $done.Count $components.Count
        foreach ($name in $components.markers | Where-Object { $_ }) { if (-not $HelperSignatures.Contains($name)) { throw "unknown marker $name" } }
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
