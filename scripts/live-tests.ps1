#Requires -Version 7.4
# The live suite (src-tauri/src/live.rs): background helpers run through Luna on
# the real CLIs, signed in to throwaway accounts in test-build/accounts. Every
# turn spends those accounts' quota; nothing here runs in CI (docs/testing.md).
#
#   ./scripts/live-tests.ps1 -ClaudeAccount live -CodexAccount live
#       build the Windows test executables into test-build\live-tests and run
#       the live tests of both providers there
#   -ClaudeAccount or -CodexAccount alone    that provider's tests only
#   -NoBuild                                  run what test-build\live-tests holds
#   -TimeoutS 240                             the most one test waits on its CLI
#
# An account is a name in test-build/accounts/<anthropic|openai>/ (make one with
# scripts/test-accounts.ps1 clone, then sign it in). Each is checked before
# anything runs: there, no junction or symlink on the way, signed in.
#
# The tests run from test-build\live-tests, which is then their data folder
# (paths.rs `data_dir`): their hub, settings and log (luna.log there) share
# nothing with a test build running beside them. The CLI is the test build's
# managed copy when it has one (test-build\claude-cli, test-build\codex-cli),
# or else whatever is on PATH, resolved here and named to the tests by its full
# path (LUNA_LIVE_CLAUDE_CLI, LUNA_LIVE_CODEX_CLI; cli.rs `live_cli`): Luna
# itself starts nothing but its managed copy.
param(
    [string]$ClaudeAccount,
    [string]$CodexAccount,
    [switch]$NoBuild,
    [int]$TimeoutS = 240
)
. "$PSScriptRoot/lib.ps1"

if (-not $IsWindows) { throw 'the live tests are built as Windows executables: run this on a Windows host' }
if (-not ($ClaudeAccount -or $CodexAccount)) { throw 'name a test account: -ClaudeAccount, -CodexAccount or both' }
# The tests would refuse each start (src-tauri/src/elevation.rs); said here
# before a build. An admin's ordinary token holds Administrators deny-only,
# so only an elevated one is in the role.
$principal = [Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
if ($principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'this terminal runs as administrator, and Luna starts no CLI so: run the live tests from an ordinary one'
}

$testBuild = Join-Path $RepoRoot 'test-build'
$dir = Join-Path $testBuild 'live-tests'

# Each provider asked for: its account's folder, the CLI found for it, the
# environment variable naming the account and the tests' name prefix.
$runs = @()
foreach ($p in @(
        @{ Name = $ClaudeAccount; Dir = 'anthropic'; Var = 'LUNA_LIVE_CLAUDE_ACCOUNT'; Prefix = 'live::claude_'; CliVar = 'LUNA_LIVE_CLAUDE_CLI'; Cli = 'claude-cli'; Bin = 'claude.exe' }
        @{ Name = $CodexAccount; Dir = 'openai'; Var = 'LUNA_LIVE_CODEX_ACCOUNT'; Prefix = 'live::codex_'; CliVar = 'LUNA_LIVE_CODEX_CLI'; Cli = 'codex-cli'; Bin = 'bin/codex.exe' }
    )) {
    if (-not $p.Name) { continue }
    $account = Join-Path $testBuild 'accounts' $p.Dir $p.Name
    foreach ($at in $testBuild, (Join-Path $testBuild 'accounts'), (Join-Path $testBuild 'accounts' $p.Dir), $account) {
        if (-not (Test-Path -LiteralPath $at -PathType Container)) {
            throw "$account is not there: make it with ./scripts/test-accounts.ps1 clone -Provider $($p.Dir) -Name $($p.Name) -Source <account folder>"
        }
        if (Test-Link $at) { throw "$at is a junction or symlink: the live tests run on throwaway accounts only" }
    }
    if (-not (Test-AccountSignedIn $p.Dir $account)) {
        throw "$account is not signed in: ./scripts/test-accounts.ps1 clone prints the one step that does it"
    }
    # The managed copy as cli.rs lays it out: `current` names the version.
    $cli = $null
    $current = Join-Path $testBuild $p.Cli 'current'
    if (Test-Path -LiteralPath $current) {
        $version = (Get-Content -LiteralPath $current -TotalCount 1).Trim()
        $exe = Join-Path $testBuild $p.Cli 'versions' $version $p.Bin
        if ($version -and (Test-Path -LiteralPath $exe)) { $cli = $exe }
    }
    if (-not $cli) {
        $onPath = Get-Command ([IO.Path]::GetFileNameWithoutExtension($p.Bin)) -CommandType Application -ErrorAction SilentlyContinue |
            Where-Object { $_.Source -like '*.exe' } | Select-Object -First 1
        if (-not $onPath) { throw "no $($p.Bin) in $testBuild\$($p.Cli) nor on PATH: start the test build once to download it" }
        $cli = $onPath.Source
    }
    Write-Host "$($p.Dir): account $account, CLI $cli"
    $runs += @{ Var = $p.Var; Account = $account; Prefix = $p.Prefix; CliVar = $p.CliVar; Cli = $cli; CliDir = Split-Path -Parent $cli }
}

if (-not $NoBuild) {
    & "$PSScriptRoot/windows-tests.ps1" -Build -Dir $dir
}
$exes = @(Get-ChildItem -LiteralPath $dir -Filter *.exe -File -ErrorAction SilentlyContinue)
if (-not $exes) { throw "no test executables in $dir - run without -NoBuild" }

Write-Warning 'the live tests spend the quota of the accounts named, a few short turns each'
$saved = @{}
$set = @{ LUNA_LIVE = '1'; LUNA_LIVE_TIMEOUT_S = "$TimeoutS"; PATH = (($runs.CliDir | Select-Object -Unique) + $env:PATH) -join [IO.Path]::PathSeparator }
foreach ($r in $runs) { $set[$r.Var] = $r.Account; $set[$r.CliVar] = $r.Cli }
$passed = 0; $failed = @()
Push-Location -LiteralPath $dir
try {
    foreach ($k in $set.Keys) {
        $saved[$k] = [Environment]::GetEnvironmentVariable($k)
        [Environment]::SetEnvironmentVariable($k, $set[$k])
    }
    foreach ($exe in $exes) {
        # Only the ignored tests, of the providers asked for, one at a time:
        # a burst of turns at once is a burst on the account's limits.
        & $exe.FullName --ignored --test-threads=1 @($runs.Prefix) 2>&1 | ForEach-Object { "$_" } | Tee-Object -Variable lines | Out-Host
        if ($LASTEXITCODE) { $failed += $exe.Name }
        foreach ($line in $lines) {
            if ($line -match '^test result: \w+\. (\d+) passed') { $passed += [int]$Matches[1] }
        }
    }
} finally {
    foreach ($k in $saved.Keys) { [Environment]::SetEnvironmentVariable($k, $saved[$k]) }
    Pop-Location
}
if ($failed) { throw "live tests failed in $($failed -join ', '); the helpers' log is $dir\luna.log" }
if (-not $passed) { throw 'no live test ran' }
Write-Host "live tests: $passed passed" -ForegroundColor Green
