#Requires -Version 7.4
# Tests for supply-chain-lib.ps1. The ignore rules and .gitleaksignore's form
# run on their own; reading deny.toml and gitleaks run in the same Docker
# images as the checks, on files and throwaway repositories of their own under
# test-build/.
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
    }

    Check 'a reason without an expiry, or with one that is no date, is refused' {
        Assert-Throws { Get-IgnoreExpiry 'x' $reason } "no 'Expires"
        Assert-Throws { Get-IgnoreExpiry 'x' "$reason Expires 2027-13-01" } 'not a yyyy-MM-dd'
    }

    Check 'an ignore past its expiry fails, one due today does not' {
        Assert-DenyIgnores @([pscustomobject]@{ Id = 'RUSTSEC-2026-0001'; Reason = "$reason Expires 2026-11-08" }) $today
        Assert-Throws { Assert-DenyIgnores @([pscustomobject]@{ Id = 'RUSTSEC-2026-0001'; Reason = "$reason Expires 2026-11-07" }) $today } 'past their expiry'
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

    Check 'a deny.toml that is not TOML fails' {
        Assert-Throws { Read-Deny broken "[advisories]`nignore = [" 2>$null } 'does not parse'
    }

    # Secrets on one line, made up here so that none is ever in the repo, in
    # a throwaway repository of its own. Returns the repository, its commit
    # and the secrets.
    function New-LeakyRepo([string]$Name) {
        $repo = Join-Path $work $Name
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
        @{ Store = (Join-Path $repo '.git'); Head = (git -C $repo rev-parse HEAD).Trim(); Secrets = @($aws, $github, $short, $aws.Substring(4), $github.Substring(4)) }
    }
    # What Invoke-Gitleaks prints and throws, as one text.
    function Get-GitleaksOutput($Leaky, [string[]]$Ignore = @()) {
        & {
            try { Invoke-Gitleaks $Leaky.Store $Leaky.Head $Ignore; 'passed' } catch { "threw: $_" }
        } *>&1 | Out-String
    }

    # Two secrets on one line, and a third: gitleaks finds them, and what it
    # prints holds none of them, nor the text around them.
    Check 'gitleaks names its findings without printing a secret or the line around it' {
        $leaky = New-LeakyRepo 'leaky'
        $output = Get-GitleaksOutput $leaky
        if ($output -notmatch 'threw: gitleaks found') { throw "gitleaks found nothing: $output" }
        if ($output -notmatch 'settings\.env:1') { throw "the finding does not name its file and line: $output" }
        foreach ($secret in $leaky.Secrets) {
            if ($output.Contains($secret)) { throw 'the output holds a secret' }
        }
    }

    Check 'gitleaks passes over exactly the findings .gitleaksignore lists' {
        $leaky = New-LeakyRepo 'ignored'
        $found = [regex]::Matches((Get-GitleaksOutput $leaky), '\(([0-9a-f]{40}:settings\.env:[a-z0-9-]+:1)\)') | ForEach-Object { $_.Groups[1].Value }
        if (@($found).Count -lt 2) { throw "expected the findings' fingerprints, got: $($found -join ', ')" }
        if (@($found | Where-Object { -not $_.StartsWith("$($leaky.Head):") })) { throw "a fingerprint is not of the scanned commit: $($found -join ', ')" }
        # All of them listed: the scan passes.
        $output = Get-GitleaksOutput $leaky $found
        if ($output -match 'threw' -or $output.Trim() -notmatch 'passed$') { throw "every finding listed should pass: $output" }
        # One left out: that one is still found, and only that one.
        $output = Get-GitleaksOutput $leaky @($found | Select-Object -Skip 1)
        if ($output -notmatch 'threw: gitleaks found 1 secret') { throw "one left out should be found alone: $output" }
        if (-not $output.Contains($found[0])) { throw "the one left out is not the one found: $output" }
        # The same findings under another commit cover nothing here.
        $other = $found | ForEach-Object { '0' * 40 + $_.Substring(40) }
        $output = Get-GitleaksOutput $leaky $other
        if ($output -notmatch "threw: gitleaks found $(@($found).Count) secret") { throw "another commit's fingerprints passed a finding: $output" }
    }

    Check '.gitleaksignore takes exact fingerprints, each under a comment of its own' {
        $file = Join-Path $work 'gitleaksignore'
        $sha = 'ab' * 20
        $read = { param($text) Set-Content -LiteralPath $file $text; Read-GitleaksIgnore $file }
        Assert-Equal (@(Read-GitleaksIgnore (Join-Path $work 'no-such-file')).Count) 0
        $both = & $read @"
# A made-up token in a test of the log redaction, not a secret.
${sha}:src/a.rs:generic-api-key:12

# Another made-up one,
# its comment on two lines.
${sha}:src/b-c/d.rs:aws-access-token:3
"@
        Assert-Equal ($both -join ' ') "${sha}:src/a.rs:generic-api-key:12 ${sha}:src/b-c/d.rs:aws-access-token:3"
        Assert-Throws { & $read "# A made-up token in a test, not a secret.`nsrc/a.rs:generic-api-key:12" } 'not an exact fingerprint'
        Assert-Throws { & $read "# A made-up token in a test, not a secret.`nabcdef12:src/a.rs:generic-api-key:12" } 'not an exact fingerprint'
        Assert-Throws { & $read "# A made-up token in a test, not a secret.`n${sha}:src/a b.rs:generic-api-key:12" } 'not an exact fingerprint'
        Assert-Throws { & $read "# A made-up token in a test, not a secret.`n${sha}:src/a.rs:generic-api-key:12'; rm -rf /'" } 'not an exact fingerprint'
        Assert-Throws { & $read "# A made-up token in a test, not a secret.`n$($sha.ToUpperInvariant()):src/a.rs:generic-api-key:12" } 'not an exact fingerprint'
        Assert-Throws { & $read "# A made-up token in a test, not a secret.`n${sha}:src/a:b.rs:generic-api-key:12" } 'not an exact fingerprint'
        Assert-Throws { Invoke-Gitleaks (Join-Path $work 'x') ('a' * 40) @("${sha}:src/a.rs:rule:1`n") } 'not an exact gitleaks fingerprint'
        Assert-Throws { & $read "${sha}:src/a.rs:generic-api-key:12" } 'no comment above it'
        Assert-Throws { & $read "# fake`n${sha}:src/a.rs:generic-api-key:12" } 'no comment above it'
        Assert-Throws { & $read "# A made-up token in a test, not a secret.`n`n${sha}:src/a.rs:generic-api-key:12" } 'no comment above it'
        Assert-Throws { & $read "# A made-up token in a test, not a secret.`n${sha}:src/a.rs:generic-api-key:12`n${sha}:src/b.rs:generic-api-key:12" } 'no comment above it'
        Assert-Throws { & $read "# A made-up token in a test, not a secret.`n${sha}:src/a.rs:generic-api-key:12`n# The same one again, made up as well.`n${sha}:src/a.rs:generic-api-key:12" } 'listed twice'
        Assert-Throws { Invoke-Gitleaks (Join-Path $work 'x') ('a' * 40) @("${sha}:src/a.rs:rule:1' x '") } 'not an exact gitleaks fingerprint'
    }

    Check "the repository's .gitleaksignore reads" {
        Read-GitleaksIgnore (Join-Path $RepoRoot '.gitleaksignore') | Out-Null
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
