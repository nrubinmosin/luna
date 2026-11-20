#Requires -Version 7.4
# Run: pwsh -NoProfile -File scripts/check-text.test.ps1
. "$PSScriptRoot/check-text.ps1"

$failures = 0
function Check([string]$Name, [scriptblock]$Test) {
    try {
        & $Test
        Write-Host "ok   $Name"
    } catch {
        $script:failures++
        Write-Host "FAIL $Name - $_" -ForegroundColor Red
    }
}

function Assert-Findings([string[]]$Actual, [string[]]$Expected = @()) {
    if ($Actual.Count -ne $Expected.Count -or ($Actual -join "`n") -cne ($Expected -join "`n")) {
        throw "Expected [$($Expected -join '; ')], got [$($Actual -join '; ')]"
    }
}

function Assert-Missing([string]$Path) {
    try { Find-FileTextIssues $Path } catch {
        if ($_.Exception.InnerException -is [IO.FileNotFoundException]) { return }
        throw
    }
    throw "Expected a missing file error for $Path"
}

function Test-TextFixture([string]$Name, $Content) {
    $path = Join-Path $work $Name
    $bytes = if ($Content -is [string]) { [Text.Encoding]::UTF8.GetBytes($Content) } else { $Content }
    [IO.File]::WriteAllBytes($path, $bytes)
    $prefix = "$(Get-RepoTextPath $path):"
    foreach ($finding in Find-FileTextIssues $path) {
        if (-not $finding.StartsWith($prefix, [StringComparison]::Ordinal)) { throw "Unexpected path in $finding" }
        $finding.Substring($prefix.Length)
    }
}

$work = Join-Path $textCheckRoot "test-build/check-text-$([guid]::NewGuid().ToString('n'))"
[void][IO.Directory]::CreateDirectory($work)
Push-Location $textCheckRoot
try {
    Check 'a Cyrillic letter is flagged where it stands' {
        Assert-Findings (Test-TextFixture cyrillic.rs "let a = 1;`nlet greeting = `"привет`";`n") @(
            '2:17 non-Latin letter "п"'
            '2:18 non-Latin letter "р"'
            '2:19 non-Latin letter "и"'
            '2:20 non-Latin letter "в"'
            '2:21 non-Latin letter "е"'
            '2:22 non-Latin letter "т"'
        )
    }

    Check 'a marker comment at the end of a line or alone above it lets the line pass' {
        Assert-Findings (Test-TextFixture marked.rs "let a = `"привет`"; // lang-check: allow`n")
        Assert-Findings (Test-TextFixture above.rs "    // lang-check: allow`nlet a = `"colour`";`n")
        Assert-Findings (Test-TextFixture above.md "<!-- lang-check: allow -->`nIn colour.`n")
        Assert-Findings (Test-TextFixture trailing.md "In colour. <!-- lang-check: allow -->`n")
        Assert-Findings (Test-TextFixture hash.toml "a = `"colour`" # lang-check: allow`n")
        Assert-Findings (Test-TextFixture block.css "/* lang-check: allow */`na { content: `"colour`"; }`n")
        Assert-Findings (Test-TextFixture doc.ts "/**`n * lang-check: allow`n * In colour.`n */`n")
        Assert-Findings (Test-TextFixture sql.txt "-- lang-check: allow`nIn colour.`n")
    }

    Check 'a trailing marker does not cover the line after it' {
        Assert-Findings (Test-TextFixture next.rs "let a = `"colour`"; // lang-check: allow`n// in colour`n") @(
            '2:7 British spelling "colour": use color'
        )
    }

    Check 'the marker inside a string is no marker' {
        $found = @(Test-TextFixture string.ts "const x = `"lang-check: allow colour привет`";`n")
        if ($found.Count -ne 7) { throw "Expected one British word and six Cyrillic letters, got $($found.Count)" }
        Assert-Findings (Test-TextFixture string-end.ts "const x = `"colour // lang-check: allow`"`n") @(
            '1:12 British spelling "colour": use color'
        )
    }

    Check 'a malformed marker is no marker' {
        foreach ($line in @(
            'colour // lang-check: allowed'
            'colour // lang-check: allow colour'
            'colour // lang-check allow'
            'colour //lang-check:allow'
            'colour lang-check: allow'
        )) {
            if (-not (Test-TextFixture malformed.rs "$line`n")) { throw "Unexpectedly allowed: $line" }
        }
        Assert-Findings (Test-TextFixture malformed-above.rs "// lang-check: allowing`ncolour`n") @(
            '2:1 British spelling "colour": use color'
        )
    }

    Check 'a British spelling is flagged with the American one' {
        Assert-Findings (Test-TextFixture colour.ts "// The Colour of a chat.`r`n") @(
            '1:8 British spelling "Colour": use color'
        )
        Assert-Findings (Test-TextFixture cancelled.ts 'it was cancelled') @(
            '1:8 British spelling "cancelled": use a single l (canceled, modeling)'
        )
        Assert-Findings (Test-TextFixture labour.md 'labour, labouring') @(
            '1:1 British spelling "labour": use labor'
            '1:9 British spelling "labouring": use labor'
        )
    }

    Check '-ise forms of a listed stem are flagged' {
        Assert-Findings (Test-TextFixture ise.md 'organise initialised recognising') @(
            '1:1 British spelling "organise": use organiz…'
            '1:10 British spelling "initialised": use initializ…'
            '1:22 British spelling "recognising": use recogniz…'
        )
    }

    Check 'American spellings and words that only contain a British one pass' {
        Assert-Findings (Test-TextFixture color.ts "// The color of a chat, canceled; analyses; emphasis; colour_scheme.`n")
    }

    Check 'words ending in -ise that are not British pass' {
        $words = 'precise promise exercise otherwise noise raise advise compromise enterprise expertise surprise revise ' +
            'supervise concise disguise'
        Assert-Findings (Test-TextFixture ise-words.md "$words`n")
    }

    Check 'Latin letters with diacritics and emoji pass' {
        Assert-Findings (Test-TextFixture latin.md "Café, naïve, Ünïcode, Łódź 🎨`n")
    }

    Check 'Unicode script, case folding, whitespace and columns match Node' {
        Assert-Findings (Test-TextFixture unicode.md "écolour𐐀 ſpelt Kcolour colourK µ colour`ncolour`u{feff}// lang-check: allow`ncolour`u{85}// lang-check: allow") @(
            '1:2 British spelling "colour": use color'
            '1:8 non-Latin letter "𐐀"'
            '1:11 British spelling "ſpelt": use spelled'
            '1:33 non-Latin letter "µ"'
            '1:35 British spelling "colour": use color'
            '3:1 British spelling "colour": use color'
        )
    }

    Check 'a text type that is not UTF-8 is a finding' {
        Assert-Findings (Test-TextFixture nul.rs "let a = 1;`0`n") @('1:1 not UTF-8 text')
        Assert-Findings (Test-TextFixture latin1.md ([byte[]](0x63, 0x61, 0x66, 0xe9, 0x0a))) @('1:1 not UTF-8 text')
        Assert-Findings (Test-TextFixture BOM.md "`u{feff}colour") @('1:1 British spelling "colour": use color')
    }

    Check 'a file of another type that is not UTF-8 is taken for binary' {
        Assert-Findings (Test-TextFixture blob.dat "colour`0привет")
        Assert-Findings (Test-TextFixture blob2.dat ([byte[]](0xff, 0xfe, 0x00)))
        Assert-Findings (Test-TextFixture known.PNG 'colour')
        Assert-Findings (Test-TextFixture unknown.dat 'colour') @('1:1 British spelling "colour": use color')
    }

    Check 'a path is taken relative to the root however it is spelled' {
        $expected = 'src-tauri/gen/x.json'
        foreach ($path in @(
            (Join-Path $textCheckRoot $expected)
            ($textCheckRoot.Replace('\', '/') + '/' + $expected)
            ($textCheckRoot + '\src-tauri\gen\x.json')
            './src-tauri/gen/x.json'
            'src-tauri\gen\x.json'
        )) {
            $actual = Get-RepoTextPath $path
            if ($actual -cne $expected) { throw "For $path, expected $expected, got $actual" }
        }
    }

    Check 'the checker and its tests are skipped by their exact paths from the root' {
        $own = @('scripts/check-text.ps1', 'scripts/check-text.test.ps1') | ForEach-Object {
            $path = Join-Path $textCheckRoot $_
            $path
            $path.Replace('/', '\')
        }
        Assert-Findings (Find-FileTextIssues $own)
        [void][IO.Directory]::CreateDirectory((Join-Path $work 'scripts'))
        Assert-Findings (Test-TextFixture scripts/check-text.ps1 "colour`n") @('1:1 British spelling "colour": use color')
        Assert-Findings (Test-TextFixture check-text.ps1.md "colour`n") @('1:1 British spelling "colour": use color')
    }

    Check 'license texts kept for other packages are skipped, the folder they are in is not' {
        Assert-Findings (Find-FileTextIssues @(
            'third-party-licenses/crates/none/LICENSE'
            'third-party-licenses/npm/@x/y/LICENSE'
            'third-party-licenses/tools/none/LICENSE'
            'third-party-licenses/installer/none/COPYING'
        ))
        Assert-Missing 'third-party-licenses/NONE.md'
        Assert-Missing 'third-party-licenses/none.json'
    }

    Check 'a symbolic link to a directory, and a directory, are passed over' {
        $target = Join-Path $work 'skill'
        [void][IO.Directory]::CreateDirectory($target)
        [IO.File]::WriteAllText((Join-Path $target 'SKILL.md'), 'In colour.')
        $link = Join-Path $work 'skill-link'
        $linkType = if ($IsWindows) { 'Junction' } else { 'SymbolicLink' }
        New-Item -ItemType $linkType -Path $link -Target $target | Out-Null
        Assert-Findings (Find-FileTextIssues @($link, $target))
        Assert-Findings (Find-FileTextIssues (Join-Path $link 'SKILL.md')) @(
            "$(Get-RepoTextPath $link)/SKILL.md:1:4 British spelling ""colour"": use color"
        )
    }

    Check 'explicit CLI paths use the caller directory and report the finding with exit 1' {
        [IO.File]::WriteAllText((Join-Path $work 'explicit file.md'), 'colour')
        $stderr = Join-Path $work 'stderr'
        Push-Location $work
        try {
            $found = @(pwsh -NoProfile -File "$PSScriptRoot/check-text.ps1" 'explicit file.md' 2> $stderr)
            $code = $LASTEXITCODE
        } finally {
            Pop-Location
        }
        Assert-Findings $found @(
            "$(Get-RepoTextPath $work)/explicit file.md:1:1 British spelling ""colour"": use color"
        )
        if ($code -ne 1) { throw "Expected exit code 1, got $code" }
        $expected = '1 finding(s). A line that must stay as it is carries "lang-check: allow".'
        if ((Get-Content -LiteralPath $stderr -Raw).Trim() -cne $expected) { throw 'Unexpected findings summary' }
    }

    Check 'the tracked tree matches the legacy Node check baseline' {
        # The pinned Node image checked the tracked files at 191d5923 with
        # exit 0 and empty stdout. Keep that expectation without needing Node.
        Push-Location $work
        try {
            $found = @(pwsh -NoProfile -File "$PSScriptRoot/check-text.ps1")
            if ($LASTEXITCODE) { throw "Tracked tree check exited $($LASTEXITCODE) with [$($found -join '; ')]" }
            Assert-Findings $found
        } finally {
            Pop-Location
        }
    }
} finally {
    Pop-Location
    Remove-Item -LiteralPath $work -Recurse -Force
}

if ($failures) { exit 1 }
Write-Host 'check-text tests passed' -ForegroundColor Green
