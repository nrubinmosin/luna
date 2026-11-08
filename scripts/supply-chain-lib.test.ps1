#Requires -Version 7.4
# Tests for supply-chain-lib.ps1. The ignore rules run on their own; reading
# deny.toml and gitleaks run in the same Docker images as the checks, on files
# and a throwaway repository of their own under test-build/.
# Run: pwsh scripts/supply-chain-lib.test.ps1 (scripts/supply-chain.ps1 runs it
# as its `tests` check)
. "$PSScriptRoot/lib.ps1"
. "$PSScriptRoot/supply-chain-lib.ps1"

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

$work = Join-Path $RepoRoot "test-build/supply-chain-tests/$([guid]::NewGuid().ToString('n'))"
$today = [datetime]'2026-11-08'
$reason = 'Linux-only, through zbus 5; no upstream on zbus 6 yet.'

# A deny.toml of the given text in a folder of its own, read as the deny check
# reads the real one.
function Read-Deny([string]$Name, [string]$Toml) {
    $dir = Join-Path $work $Name
    New-Item -ItemType Directory -Force $dir | Out-Null
    $file = Join-Path $dir 'deny.toml'
    Set-Content -LiteralPath $file ($Toml -replace "`r`n", "`n") -NoNewline
    @(Get-DenyIgnores $file)
}

try {
    Check 'a reason with an expiry to come passes, and gives its date' {
        Assert-Equal ((Get-IgnoreExpiry 'x' "$reason Expires 2027-02-08").ToString('yyyy-MM-dd')) '2027-02-08'
    }

    Check 'a blank, whitespace or token reason is refused' {
        Assert-Throws { Get-IgnoreExpiry 'x' 'Expires 2027-02-08' } 'no reason'
        Assert-Throws { Get-IgnoreExpiry 'x' "   `t Expires 2027-02-08" } 'no reason'
        Assert-Throws { Get-IgnoreExpiry 'x' 'not needed. Expires 2027-02-08' } 'no reason'
        Assert-Throws { Get-IgnoreExpiry 'x' "   `t  " '2027-02-08' } 'no reason'
        Assert-Throws { Get-IgnoreExpiry 'x' $null '2027-02-08' } 'no reason'
    }

    Check 'a reason without an expiry, or with one that is no date, is refused' {
        Assert-Throws { Get-IgnoreExpiry 'x' $reason } "no 'Expires"
        Assert-Throws { Get-IgnoreExpiry 'x' "$reason Expires 2027-13-01" } 'not a yyyy-MM-dd'
        Assert-Throws { Get-IgnoreExpiry 'x' $reason 'soon' } 'not a yyyy-MM-dd'
    }

    Check 'an ignore past its expiry fails, one due today does not' {
        Assert-DenyIgnores @([pscustomobject]@{ Id = 'RUSTSEC-2026-0001'; Reason = "$reason Expires 2026-11-08" }) $today
        Assert-Throws { Assert-DenyIgnores @([pscustomobject]@{ Id = 'RUSTSEC-2026-0001'; Reason = "$reason Expires 2026-11-07" }) $today } 'past their expiry'
        $npm = [pscustomobject]@{ id = 'GHSA-aaaa-bbbb-cccc'; package = 'x'; reason = $reason; expires = '2026-11-07' }
        Assert-Throws { Assert-NpmIgnores @($npm) $today } 'past their expiry'
    }

    Check 'an npm ignore with a blank field is refused' {
        $good = @{ id = 'GHSA-aaaa-bbbb-cccc'; package = 'x'; reason = $reason; expires = '2027-02-08' }
        Assert-NpmIgnores @([pscustomobject]$good) $today
        foreach ($field in 'id', 'package', 'reason', 'expires') {
            $bad = $good.Clone(); $bad[$field] = '   '
            Assert-Throws { Assert-NpmIgnores @([pscustomobject]$bad) $today } $(if ($field -eq 'reason') { 'no reason' } else { "without '$field'" })
        }
    }

    Check 'the repo''s deny.toml reads, and its ignores pass' {
        $entries = @(Get-DenyIgnores (Join-Path $RepoRoot 'src-tauri/deny.toml'))
        if (-not $entries) { throw 'no ignores read' }
        Assert-DenyIgnores $entries $today
    }

    Check 'a bare id in the ignore array is seen, and refused for want of a reason' {
        $entries = Read-Deny bare @"
[advisories]
ignore = [
    { id = "RUSTSEC-2026-0001", reason = "$reason Expires 2027-02-08" },
    "RUSTSEC-2026-0002",
]
"@
        Assert-Equal $entries.Count 2
        Assert-Equal $entries[1].Id 'RUSTSEC-2026-0002'
        Assert-Throws { Assert-DenyIgnores $entries $today } 'no reason'
    }

    Check 'inline tables laid out otherwise, and arrays of tables, are seen' {
        $entries = Read-Deny compact @"
[advisories]
ignore = [{id="RUSTSEC-2026-0001",reason="Expires 2027-02-08"},
  {   reason = '$reason Expires 2027-02-08'  ,  id = 'RUSTSEC-2026-0002'   }]
"@
        Assert-Equal ($entries.Id -join ' ') 'RUSTSEC-2026-0001 RUSTSEC-2026-0002'
        Assert-Throws { Assert-DenyIgnores $entries $today } 'RUSTSEC-2026-0001 has no reason'
        $entries = Read-Deny tables @"
[[advisories.ignore]]
id = "RUSTSEC-2026-0003"

[[advisories.ignore]]
id = "RUSTSEC-2026-0004"
reason = "$reason Expires 2027-02-08"
"@
        Assert-Equal ($entries.Id -join ' ') 'RUSTSEC-2026-0003 RUSTSEC-2026-0004'
        Assert-Throws { Assert-DenyIgnores $entries $today } 'RUSTSEC-2026-0003 has no reason'
    }

    # As the workflow runs it: GitHub's pwsh step ends on `exit $LASTEXITCODE`,
    # and a native command that failed before (here as pnpm audit does on any
    # advisory) must not decide the step for the script.
    Check 'the audit check''s exit code is its verdict: 0 below the bar, 1 on a high' {
        New-Item -ItemType Directory -Force $work | Out-Null
        $advisory = { param($id, $severity) @{ github_advisory_id = $id; severity = $severity; module_name = 'pkg'; vulnerable_versions = '<2'; patched_versions = '>=2'; findings = @(@{ paths = @('.>pkg') }) } }
        $report = {
            param($name, [hashtable[]]$advisories)
            $all = [ordered]@{}; $i = 0
            foreach ($a in $advisories) { $all["$((++$i))"] = $a }
            $path = Join-Path $work "$name.json"
            @{ advisories = $all; metadata = @{ totalDependencies = 1; vulnerabilities = @{ critical = 0; high = 0; moderate = 0; low = 0 } } } | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $path
            $path
        }
        $step = {
            param($path)
            $script = Join-Path $PSScriptRoot 'supply-chain.ps1'
            pwsh -NoProfile -Command "`$ErrorActionPreference = 'stop'; pwsh -NoProfile -Command 'exit 1'; & '$script' -Check audit -AuditReport '$path' *> `$null; if ((Test-Path -LiteralPath variable:\LASTEXITCODE)) { exit `$LASTEXITCODE }" *> $null
            $LASTEXITCODE
        }
        Assert-Equal (& $step (& $report moderate @((& $advisory 'GHSA-aaaa-bbbb-cccc' moderate), (& $advisory 'GHSA-dddd-eeee-ffff' low)))) 0
        Assert-Equal (& $step (& $report high @((& $advisory 'GHSA-aaaa-bbbb-cccc' moderate), (& $advisory 'GHSA-gggg-hhhh-iiii' high)))) 1
        Assert-Equal (& $step (& $report critical @(& $advisory 'GHSA-jjjj-kkkk-llll' critical))) 1
    }

    Check 'a deny.toml that is not TOML fails' {
        Assert-Throws { Read-Deny broken "[advisories]`nignore = [" 2>$null } 'does not parse'
    }

    # Two secrets on one line, made up here so that none is ever in the repo:
    # gitleaks finds them, and what it prints holds neither, nor the text
    # around them.
    Check 'gitleaks names its findings without printing a secret or the line around it' {
        $repo = Join-Path $work 'leaky'
        New-Item -ItemType Directory -Force $repo | Out-Null
        $upper = 'ABCDEFGHIJKLMNOPQRSTUVWXYZ234567'
        $alnum = 'abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789'
        $aws = 'AKIA' + [Security.Cryptography.RandomNumberGenerator]::GetString($upper, 16)
        $github = 'ghp_' + [Security.Cryptography.RandomNumberGenerator]::GetString($alnum, 36)
        $short = [Security.Cryptography.RandomNumberGenerator]::GetString($alnum, 12)
        Set-Content -LiteralPath (Join-Path $repo 'settings.env') "aws_key=$aws token=$short gh=$github`n" -NoNewline
        Invoke-Checked 'git init' { git -C $repo init -q }
        Invoke-Checked 'git add' { git -C $repo add settings.env }
        Invoke-Checked 'git commit' { git -C $repo -c user.name=test -c user.email=test@example.invalid -c commit.gpgsign=false commit -q -m 'A file with secrets' }
        $head = (git -C $repo rev-parse HEAD).Trim()
        $output = & {
            try { Invoke-Gitleaks (Join-Path $repo '.git') $head } catch { "threw: $_" }
        } *>&1 | Out-String
        if ($output -notmatch 'threw: gitleaks found') { throw "gitleaks found nothing: $output" }
        if ($output -notmatch 'settings\.env:1') { throw "the finding does not name its file and line: $output" }
        foreach ($secret in $aws, $github, $short, $aws.Substring(4), $github.Substring(4)) {
            if ($output.Contains($secret)) { throw 'the output holds a secret' }
        }
    }

    Check 'gitleaks passes a clean history, and refuses a range that is not commit ids' {
        $repo = Join-Path $work 'clean'
        New-Item -ItemType Directory -Force $repo | Out-Null
        Set-Content -LiteralPath (Join-Path $repo 'readme.txt') 'nothing to see' -NoNewline
        Invoke-Checked 'git init' { git -C $repo init -q }
        Invoke-Checked 'git add' { git -C $repo add readme.txt }
        Invoke-Checked 'git commit' { git -C $repo -c user.name=test -c user.email=test@example.invalid -c commit.gpgsign=false commit -q -m 'A clean file' }
        Invoke-Gitleaks (Join-Path $repo '.git') (git -C $repo rev-parse HEAD).Trim()
        Assert-Throws { Invoke-Gitleaks (Join-Path $repo '.git') 'HEAD; rm -rf /' } 'one commit id'
    }
} finally {
    Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
}

if ($failures) {
    Write-Host "$failures test(s) failed" -ForegroundColor Red
    exit 1
}
Write-Host 'supply-chain-lib tests passed' -ForegroundColor Green
