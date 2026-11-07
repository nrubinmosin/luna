#Requires -Version 7.4
# Throwaway accounts for the test build and the live tests, in test-build/accounts
# (ignored by git), the test build's default accounts folder.
#
#   ./scripts/test-accounts.ps1 clone -Source <account folder> -Provider anthropic|openai [-Name x] [-WithCredentials]
#       an independent copy at test-build/accounts/<provider>/<name> (the source's
#       folder name by default), without its login unless -WithCredentials
#   ./scripts/test-accounts.ps1 list            the accounts there, and which are signed in
#   ./scripts/test-accounts.ps1 clean [-Name x] [-Provider p]
#       deletes one account there, or all of them
#   ./scripts/test-accounts.ps1 self-test       the three above, on a fake account it makes
#
# A copy is plain files, never a link: what a test writes into its account must
# not reach the source. So a source with a junction or symlink anywhere in it,
# or on the way to it, is refused, as is a destination reached through one, and
# `clean` deletes nothing through a link either.
#
# The conversations, logs and prompt history are left behind (CLAUDE_SKIP,
# CODEX_SKIP): bulky, personal, and no test needs them. The login is left behind
# too unless asked for. Copied, it is the same OAuth grant in two folders, and
# its refresh tokens rotate: whichever folder refreshes first leaves the other
# holding a token that no longer works, so a test run can sign the original out.
param(
    [Parameter(Position = 0, Mandatory)]
    [ValidateSet('clone', 'list', 'clean', 'self-test')]
    [string]$Command,
    [string]$Source,
    [ValidateSet('anthropic', 'openai')]
    [string]$Provider,
    [string]$Name,
    [switch]$WithCredentials
)
. "$PSScriptRoot/lib.ps1"

$TestBuild = Join-Path $RepoRoot 'test-build'
$AccountsRoot = Join-Path $TestBuild 'accounts'

# What a clone leaves out, by top-level name in the account folder.
$CLAUDE_SKIP = @('projects', 'file-history', 'shell-snapshots', 'debug', 'logs', 'history.jsonl')
$CODEX_SKIP = @('sessions', 'archived_sessions', 'log', 'logs', 'history.jsonl')
$CREDENTIALS = @{ anthropic = '.credentials.json'; openai = 'auth.json' }

function Get-Credentials([string]$Provider) { $CREDENTIALS[$Provider] }

# The one step that signs a copy in, for the copy at $Path.
function Get-LoginStep([string]$Provider, [string]$Path) {
    if ($Provider -eq 'anthropic') {
        "`$env:CLAUDE_CONFIG_DIR = '$Path'; claude   # then /login, and /exit once signed in"
    } else {
        "`$env:CODEX_HOME = '$Path'; codex login"
    }
}

# Throws unless every folder from test-build/ down to $Path that exists is a
# plain folder: a link on the way would lead a write or a delete elsewhere.
function Assert-NoLinkDown([string]$Path) {
    $full = [IO.Path]::GetFullPath($Path)
    $base = [IO.Path]::GetFullPath($TestBuild)
    if (-not (Test-Inside $full $base)) { throw "$full is not in $base" }
    $at = $full
    while ($true) {
        if (Test-Link $at) { throw "$at is a junction or symlink: refused, it may lead to a real account" }
        if ($at.TrimEnd('\', '/') -eq $base.TrimEnd('\', '/')) { break }
        $at = Split-Path -Parent $at
    }
}

# Whether $Path is $Dir or inside it.
function Test-Inside([string]$Path, [string]$Dir) {
    $case = if ($IsWindows) { [StringComparison]::OrdinalIgnoreCase } else { [StringComparison]::Ordinal }
    $p = $Path.TrimEnd('\', '/') + [IO.Path]::DirectorySeparatorChar
    $d = $Dir.TrimEnd('\', '/') + [IO.Path]::DirectorySeparatorChar
    $p.StartsWith($d, $case)
}

# Throws if $Path, a folder above it, or anything inside it is a link.
function Assert-NoLinkIn([string]$Path) {
    $at = [IO.Path]::GetFullPath($Path)
    while ($at) {
        if (Test-Link $at) { throw "$at is a junction or symlink: refused, a copy is plain files" }
        $at = Split-Path -Parent $at
    }
    $linked = @(Get-ChildItem -LiteralPath $Path -Recurse -Force -Attributes ReparsePoint -ErrorAction Stop)
    if ($linked) { throw "$Path holds junctions or symlinks, refused: $($linked.FullName -join ', ')" }
}

# The account names accounts.rs takes (`validate_name`).
function Assert-AccountName([string]$Name) {
    if (-not $Name -or $Name.Length -gt 64 -or $Name -in '.', '..' -or $Name -match '[\\/:*?"<>|\x00-\x1f]') {
        throw "'$Name' is not an account name Luna takes (1-64 characters, none of \ / : * ? `" < > |)"
    }
}

function Invoke-Clone([string]$Source, [string]$Provider, [string]$Name, [bool]$WithCredentials) {
    if (-not $Source -or -not $Provider) { throw 'clone needs -Source and -Provider' }
    $src = [IO.Path]::GetFullPath($Source, (Get-Location).Path)
    if (-not (Test-Path -LiteralPath $src -PathType Container)) { throw "$src is not a folder" }
    if (-not $Name) { $Name = Split-Path -Leaf $src }
    Assert-AccountName $Name
    $dest = Join-Path $AccountsRoot $Provider $Name
    if (Test-Inside $dest $src) { throw "$dest would be inside its own source $src" }
    if (Test-Path -LiteralPath $dest) { throw "$dest already exists: clean -Name $Name first" }
    Assert-NoLinkIn $src
    New-Item -ItemType Directory -Force (Split-Path -Parent $dest) | Out-Null
    Assert-NoLinkDown (Split-Path -Parent $dest)

    $skip = if ($Provider -eq 'anthropic') { $CLAUDE_SKIP } else { $CODEX_SKIP }
    $cred = Get-Credentials $Provider
    if (-not $WithCredentials) { $skip += $cred }
    $looks = if ($Provider -eq 'anthropic') { '.claude.json', '.credentials.json', 'settings.json' } else { 'config.toml', 'auth.json' }
    if (-not ($looks | Where-Object { Test-Path -LiteralPath (Join-Path $src $_) })) {
        Write-Warning "$src has none of $($looks -join ', '): is it a $Provider account folder?"
    }

    New-Item -ItemType Directory $dest | Out-Null
    $files = 0; $bytes = 0; $left = @()
    foreach ($entry in Get-ChildItem -LiteralPath $src -Force) {
        if ($entry.Name -in $skip) { $left += $entry.Name; continue }
        $to = Join-Path $dest $entry.Name
        if ($entry.PSIsContainer) {
            Copy-Item -LiteralPath $entry.FullName -Destination $to -Recurse -Force
            $got = @(Get-ChildItem -LiteralPath $to -Recurse -Force -File)
        } else {
            Copy-Item -LiteralPath $entry.FullName -Destination $to -Force
            $got = @(Get-Item -LiteralPath $to -Force)
        }
        $files += $got.Count
        $bytes += ($got | Measure-Object Length -Sum).Sum
    }
    Write-Host "cloned $src -> $dest ($files files, $([math]::Round($bytes / 1MB, 1)) MB)"
    if ($left) { Write-Host "left out: $($left -join ', ')" }
    if ($WithCredentials) {
        Write-Warning ("copied the login ($cred). OAuth refresh tokens rotate: the first of the two folders to refresh " +
            "leaves the other with a dead token, so using this copy can sign the original out, and the other way round. " +
            "Sign the copy in on its own instead where you can.")
    } else {
        Write-Host 'not signed in; sign it in once with:'
        Write-Host "  $(Get-LoginStep $Provider $dest)"
    }
    $dest
}

function Get-TestAccounts {
    foreach ($p in 'anthropic', 'openai') {
        $dir = Join-Path $AccountsRoot $p
        if (-not (Test-Path -LiteralPath $dir)) { continue }
        foreach ($a in Get-ChildItem -LiteralPath $dir -Directory -Force) {
            [pscustomobject]@{
                Provider = $p
                Name     = $a.Name
                SignedIn = Test-AccountSignedIn $p $a.FullName
                Link     = Test-Link $a.FullName
                Path     = $a.FullName
            }
        }
    }
}

function Invoke-Clean([string]$Name, [string]$Provider) {
    if (-not (Test-Path -LiteralPath $AccountsRoot)) { return }
    Assert-NoLinkDown $AccountsRoot
    $targets = @(Get-TestAccounts | Where-Object { (-not $Name -or $_.Name -eq $Name) -and (-not $Provider -or $_.Provider -eq $Provider) })
    if ($Name -and -not $targets) { throw "no test account named $Name in $AccountsRoot" }
    if ($Name -and -not $Provider -and $targets.Count -gt 1) { throw "$Name is an account of both providers: say -Provider" }
    # All checked before any is deleted: a refusal leaves everything as it was.
    foreach ($t in $targets) {
        Assert-NoLinkDown $t.Path
        Assert-NoLinkIn $t.Path
    }
    foreach ($t in $targets) {
        Remove-Item -LiteralPath $t.Path -Recurse -Force
        Write-Host "deleted $($t.Path)"
    }
}

# clone, list and clean on a fake account folder made for it, under test-build/,
# and taken away again: never a real account.
function Invoke-SelfTest {
    $id = [guid]::NewGuid().ToString('n').Substring(0, 8)
    $work = Join-Path $TestBuild "test-accounts-selftest-$id"
    $names = @("selftest-$id", "selftest-$id-creds")
    $link = Join-Path $AccountsRoot 'anthropic' "selftest-$id-link"
    $fails = [Collections.Generic.List[string]]::new()
    function Check([bool]$Ok, [string]$What) {
        if ($Ok) { Write-Host "ok   $What" } else { Write-Host "FAIL $What" -ForegroundColor Red; $fails.Add($What) }
    }
    function Write-Fake([string]$Path, [string]$Text) {
        New-Item -ItemType Directory -Force (Split-Path -Parent $Path) | Out-Null
        Set-Content -LiteralPath $Path -Value $Text -NoNewline
    }
    try {
        $claude = Join-Path $work 'claude'
        Write-Fake (Join-Path $claude '.credentials.json') '{"claudeAiOauth":{"refreshToken":"fake"}}'
        Write-Fake (Join-Path $claude '.claude.json') '{}'
        Write-Fake (Join-Path $claude 'settings.json') '{}'
        Write-Fake (Join-Path $claude 'plugins/x/plugin.json') '{}'
        Write-Fake (Join-Path $claude 'projects/p/s.jsonl') '{}'
        Write-Fake (Join-Path $claude 'history.jsonl') '{}'
        $codex = Join-Path $work 'codex'
        Write-Fake (Join-Path $codex 'auth.json') '{"tokens":{"id_token":"fake"}}'
        Write-Fake (Join-Path $codex 'config.toml') 'model = "x"'
        Write-Fake (Join-Path $codex 'skills/s/SKILL.md') 'x'
        Write-Fake (Join-Path $codex 'sessions/2026/01/r.jsonl') '{}'
        Write-Fake (Join-Path $codex 'log/codex-tui.log') 'x'

        $a = Invoke-Clone $claude 'anthropic' $names[0] $false | Select-Object -Last 1
        Check (Test-Path (Join-Path $a 'plugins/x/plugin.json')) 'clone copies the account tree'
        Check (-not (Test-Path (Join-Path $a '.credentials.json'))) 'clone leaves the Claude login out by default'
        Check (-not (Test-Path (Join-Path $a 'projects')) -and -not (Test-Path (Join-Path $a 'history.jsonl'))) 'clone leaves Claude projects/ and history out'
        $o = Invoke-Clone $codex 'openai' $names[0] $false | Select-Object -Last 1
        Check (Test-Path (Join-Path $o 'skills/s/SKILL.md')) 'clone copies a Codex account'
        Check (-not (Test-Path (Join-Path $o 'auth.json'))) 'clone leaves the Codex login out by default'
        Check (-not (Test-Path (Join-Path $o 'sessions')) -and -not (Test-Path (Join-Path $o 'log'))) 'clone leaves Codex sessions/ and log/ out'
        $c = Invoke-Clone $claude 'anthropic' $names[1] $true 3>$null | Select-Object -Last 1
        Check (Test-Path (Join-Path $c '.credentials.json')) '-WithCredentials copies the login'
        $listed = @(Get-TestAccounts | Where-Object Name -in $names)
        Check ($listed.Count -eq 3) 'list shows the clones'
        Check (($listed | Where-Object { $_.Name -eq $names[1] }).SignedIn -and -not ($listed | Where-Object { $_.Name -eq $names[0] -and $_.SignedIn })) 'list tells signed in from not'

        $threw = try { Invoke-Clone $claude 'anthropic' $names[0] $false | Out-Null; $false } catch { $true }
        Check $threw 'clone refuses an account that exists'
        $linked = Join-Path $work 'linked'
        New-Item -ItemType Directory $linked | Out-Null
        $linkType = if ($IsWindows) { 'Junction' } else { 'SymbolicLink' }
        New-Item -ItemType $linkType -Path (Join-Path $linked 'inner') -Target $codex | Out-Null
        $threw = try { Invoke-Clone $linked 'openai' "selftest-$id-linked" $false | Out-Null; $false } catch { $_.Exception.Message -match 'junctions or symlinks' }
        Check ($threw -and -not (Test-Path (Join-Path $AccountsRoot 'openai' "selftest-$id-linked"))) 'clone refuses a source with a link in it'

        New-Item -ItemType $linkType -Path $link -Target $claude | Out-Null
        $threw = try { Invoke-Clean "selftest-$id-link" 'anthropic'; $false } catch { $_.Exception.Message -match 'junction or symlink' }
        Check ($threw -and (Test-Path (Join-Path $claude '.credentials.json'))) 'clean refuses a link and leaves its target alone'
        [IO.Directory]::Delete($link)

        Invoke-Clean $names[0] 'anthropic'
        Check (-not (Test-Path $a) -and (Test-Path $o)) 'clean -Name -Provider deletes that one only'
        Invoke-Clean $names[0] ''
        Invoke-Clean $names[1] ''
        Check (-not (Get-TestAccounts | Where-Object Name -in $names)) 'clean -Name deletes the rest'
        Check (Test-Path (Join-Path $claude '.credentials.json')) 'the source is untouched'
    } finally {
        if (Test-Path -LiteralPath $link) { [IO.Directory]::Delete($link) }
        foreach ($inner in Get-ChildItem -LiteralPath $work -Recurse -Force -Attributes ReparsePoint -ErrorAction SilentlyContinue) {
            [IO.Directory]::Delete($inner.FullName)
        }
        foreach ($p in 'anthropic', 'openai') {
            foreach ($n in $names) {
                $d = Join-Path $AccountsRoot $p $n
                if ((Test-Path -LiteralPath $d) -and -not (Test-Link $d)) { Remove-Item -LiteralPath $d -Recurse -Force }
            }
        }
        if (Test-Path -LiteralPath $work) { Remove-Item -LiteralPath $work -Recurse -Force }
    }
    if ($fails) { throw "test-accounts self-test: $($fails.Count) failed" }
    Write-Host 'test-accounts self-test: all passed' -ForegroundColor Green
}

switch ($Command) {
    'clone' { Invoke-Clone $Source $Provider $Name $WithCredentials.IsPresent | Out-Null }
    'list' { Get-TestAccounts | Format-Table Provider, Name, SignedIn, Link, Path -AutoSize }
    'clean' { Invoke-Clean $Name $Provider }
    'self-test' { Invoke-SelfTest }
}
