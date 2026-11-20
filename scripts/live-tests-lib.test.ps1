#Requires -Version 7.4
# Tests for live-tests-lib.ps1, on made-up CLI trees of their own under
# test-build/.
# Run: pwsh scripts/live-tests-lib.test.ps1 (the gate runs it with the scripts group)
. "$PSScriptRoot/lib.ps1"
. "$PSScriptRoot/live-tests-lib.ps1"

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

$work = Join-Path $RepoRoot "test-build/live-tests-lib-tests/$([guid]::NewGuid().ToString('n'))"

# A CLI folder with an executable in each of `$Versions` and, when given,
# `state.json` and 0.19's `current` as written.
function New-Tree([string]$Name, [string]$Bin, [string[]]$Versions, [string]$State, [string]$Current) {
    $root = Join-Path $work $Name
    foreach ($v in $Versions) {
        $exe = Join-Path $root 'versions' $v $Bin
        New-Item -ItemType Directory -Force -Path (Split-Path -Parent $exe) | Out-Null
        Set-Content -LiteralPath $exe -Value 'exe'
    }
    New-Item -ItemType Directory -Force -Path $root | Out-Null
    if ($State) { Set-Content -LiteralPath (Join-Path $root 'state.json') -Value $State }
    if ($Current) { Set-Content -LiteralPath (Join-Path $root 'current') -Value $Current }
    $root
}

try {
    Check 'state.json names the folder, not the version' {
        # A version fetched again beside an unrecorded one gets a suffixed folder.
        $state = '{ "current": { "dir": "2.1.0~1", "version": "2.1.0" }, "previous": { "dir": "2.0.0", "version": "2.0.0" } }'
        $root = New-Tree 'claude-suffixed' 'claude.exe' '2.0.0', '2.1.0~1' $state
        Assert-Equal (Get-ManagedCli $root 'claude.exe') (Join-Path $root 'versions' '2.1.0~1' 'claude.exe')
    }
    Check 'a nested executable, as Codex has' {
        $root = New-Tree 'codex' 'bin/codex.exe' '0.50.0' '{ "current": { "dir": "0.50.0", "version": "0.50.0" } }'
        Assert-Equal (Get-ManagedCli $root 'bin/codex.exe') (Join-Path $root 'versions' '0.50.0' 'bin/codex.exe')
    }
    Check 'state.json wins over a current left beside it' {
        $root = New-Tree 'both' 'claude.exe' '1.0.0', '2.0.0' '{ "current": { "dir": "2.0.0", "version": "2.0.0" } }' '1.0.0'
        Assert-Equal (Get-ManagedCli $root 'claude.exe') (Join-Path $root 'versions' '2.0.0' 'claude.exe')
    }
    Check "0.19's current, before the tree migrates" {
        $root = New-Tree 'legacy' 'claude.exe' '1.0.0' -Current '1.0.0'
        Assert-Equal (Get-ManagedCli $root 'claude.exe') (Join-Path $root 'versions' '1.0.0' 'claude.exe')
    }
    Check 'nothing current yet' {
        $root = New-Tree 'empty-record' 'claude.exe' '1.0.0' '{ "bad": { "1.0.0": { "why": "x", "by": "probe" } } }'
        Assert-Equal (Get-ManagedCli $root 'claude.exe') $null
    }
    Check 'no tree at all' {
        Assert-Equal (Get-ManagedCli (Join-Path $work 'absent') 'claude.exe') $null
    }
    Check 'a record that does not parse names nothing' {
        $root = New-Tree 'garbled' 'claude.exe' '1.0.0' '{ "current": ' '1.0.0'
        Assert-Equal (Get-ManagedCli $root 'claude.exe' 3>$null) $null
    }
    Check 'a folder named but gone' {
        $root = New-Tree 'gone' 'claude.exe' '1.0.0' '{ "current": { "dir": "2.0.0", "version": "2.0.0" } }'
        Assert-Equal (Get-ManagedCli $root 'claude.exe') $null
    }
    Check 'a name that is not a plain folder name' {
        # Each would reach the executable of the tree beside it.
        New-Tree 'outside' 'claude.exe' '1.0.0' | Out-Null
        $i = 0
        foreach ($dir in '..', '../../outside/versions/1.0.0', '..\\..\\outside\\versions\\1.0.0') {
            $root = New-Tree "escape-$(($i++))" 'claude.exe' @() "{ ""current"": { ""dir"": ""$dir"", ""version"": ""1.0.0"" } }"
            Assert-Equal (Get-ManagedCli $root 'claude.exe') $null
        }
    }
} finally {
    Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
}

if ($failures) {
    Write-Host "$failures test(s) failed" -ForegroundColor Red
    exit 1
}
Write-Host 'live-tests-lib tests passed' -ForegroundColor Green
