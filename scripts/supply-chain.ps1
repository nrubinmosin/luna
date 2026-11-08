#Requires -Version 7.4
# The supply-chain checks, each in Docker on pinned tools, the same ones
# .github/workflows/security.yml runs:
#
#   deny     cargo-deny over the crate's tree (src-tauri/deny.toml): RustSec
#            advisories, licenses, bans and sources
#   audit    pnpm audit over the npm tree, production and dev: a high or
#            critical advisory fails it unless pnpm-audit-ignores.json names it
#   secrets  gitleaks over the git history, secrets redacted in its output
#
#   ./scripts/supply-chain.ps1                         all three
#   ./scripts/supply-chain.ps1 -Check deny,audit       some of them
#   ./scripts/supply-chain.ps1 -Check secrets -Range <base>..<head>
#                                                      only the commits of a range
#
# Unlike the gate (scripts/gate.ps1) these need the network, and their answer
# changes without any change to the repo: a new advisory fails a tree that
# passed yesterday. So the gate stays hermetic and this runs beside it.
#
# Every check runs, and the script ends on a summary of all of them: exit 1 when
# any failed. An ignore, of an advisory in either tree, ends on an expiry date,
# and a check fails once that date has passed.
param(
    [ValidateSet('deny', 'audit', 'secrets')]
    [string[]]$Check = @('deny', 'audit', 'secrets'),
    # What gitleaks scans, as git log takes it: a range, or a commit for the
    # whole history reachable from it.
    [string]$Range = 'HEAD'
)
. "$PSScriptRoot/lib.ps1"

# cargo-deny's release binary, checked against the SHA-256 pinned here before
# it runs: built from its own lockfile by its maintainers, where a `cargo
# install` here would compile it and its whole tree on every CI run.
$CargoDeny = '0.20.2'
$CargoDenySha256 = '9f12ed4c49936e09b48bf862b595cde2fe64fcbd9d74dfacac6131ca824c8d5f'
# gitleaks' official image, by tag for people and digest for the image itself.
$Gitleaks = 'ghcr.io/gitleaks/gitleaks:v8.30.1@sha256:c00b6bd0aeb3071cbcb79009cb16a60dd9e0a7c60e2be9ab65d25e6bc8abbb7f'

$today = [datetime]::UtcNow.Date

# Whether an ignore's expiry has passed; throws on a date that does not parse,
# so a typo cannot keep an ignore forever.
function Test-Expired([string]$What, [string]$Date) {
    $parsed = [datetime]::MinValue
    if (-not [datetime]::TryParseExact($Date, 'yyyy-MM-dd', [cultureinfo]::InvariantCulture, 'None', [ref]$parsed)) {
        throw "$What has no expiry date (yyyy-MM-dd), or one that does not parse: '$Date'"
    }
    $parsed -lt $today
}

# The release archive, downloaded once into test-build/ and checked against the
# pin every time it is used. Returns the folder holding it.
function Get-CargoDeny {
    $dir = Join-Path $RepoRoot "test-build/supply-chain/cargo-deny-$CargoDeny"
    $name = "cargo-deny-$CargoDeny-x86_64-unknown-linux-musl.tar.gz"
    $file = Join-Path $dir $name
    if (-not (Test-Path -LiteralPath $file)) {
        New-Item -ItemType Directory -Force $dir | Out-Null
        Invoke-WebRequest "https://github.com/EmbarkStudios/cargo-deny/releases/download/$CargoDeny/$name" -OutFile $file
    }
    $hash = (Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($hash -ne $CargoDenySha256) {
        Remove-Item -LiteralPath $file
        throw "$name has SHA-256 $hash, not the pinned $CargoDenySha256; removed it"
    }
    $dir
}

# deny.toml's ignores carry their expiry in the reason, which is all cargo-deny
# lets an ignore say: '... Expires YYYY-MM-DD'.
function Assert-DenyIgnores {
    $toml = Get-Content (Join-Path $RepoRoot 'src-tauri/deny.toml') -Raw
    $entries = [regex]::Matches($toml, '(?m)^\s*\{\s*id\s*=\s*"([^"]+)"\s*,\s*reason\s*=\s*"([^"]*)"\s*\}')
    if ($entries.Count -ne [regex]::Matches($toml, '(?m)^\s*\{\s*id\s*=').Count) {
        throw 'src-tauri/deny.toml has an ignore not written as { id = "...", reason = "... Expires YYYY-MM-DD" } on one line'
    }
    $expired = foreach ($entry in $entries) {
        $id = $entry.Groups[1].Value
        $date = if ($entry.Groups[2].Value -match 'Expires (\S+)$') { $Matches[1] } else { '' }
        if (Test-Expired "deny.toml's ignore of $id" $date) { "$id (expired $date)" }
    }
    if ($expired) { throw "src-tauri/deny.toml ignores past their expiry: $($expired -join ', '); review each and renew or drop it" }
}

# In the Rust image the Windows toolchain image builds on, as scripts/sbom.ps1
# does: cargo-deny runs cargo metadata, and fetches the advisory database with
# the git CLI, which the slim image lacks. RUSTUP_TOOLCHAIN names the image's
# own toolchain, so rustup does not fetch what rust-toolchain.toml lists on top.
function Invoke-Deny {
    Assert-DenyIgnores
    $rust = Get-BaseImages | Where-Object { $_.File -eq 'docker/windows-build.Dockerfile' -and $_.Repo -eq 'rust' }
    if (@($rust).Count -ne 1) { throw 'docker/windows-build.Dockerfile should have one FROM rust: line' }
    $tool = Get-CargoDeny
    $archive = "cargo-deny-$CargoDeny-x86_64-unknown-linux-musl"
    Invoke-Checked 'cargo deny' {
        docker run --rm `
          -e RUSTUP_TOOLCHAIN=$($rust.Tag -replace '-.*$', '') `
          -e CARGO_TERM_COLOR=never `
          -v "${RepoRoot}:/app:ro" `
          -v "${tool}:/tool:ro" `
          -v luna-cargo-registry:/usr/local/cargo/registry `
          -w /app/src-tauri `
          $rust.Ref `
          bash -c (ConvertTo-BashScript @"
set -e
tar -xzf /tool/$archive.tar.gz -C /tmp
/tmp/$archive/cargo-deny --locked check
"@) | Out-Host
    }
}

# pnpm audit reports every advisory of the lockfile's packages, dev ones too;
# this decides on its JSON, so that an ignore can carry a reason and an expiry
# (pnpm's own auditConfig.ignoreGhsas takes neither).
function Invoke-Audit {
    $ignoreFile = Join-Path $RepoRoot 'pnpm-audit-ignores.json'
    $ignores = @((Get-Content $ignoreFile -Raw | ConvertFrom-Json).ignores)
    $expired = foreach ($ignore in $ignores) {
        foreach ($field in 'id', 'package', 'reason', 'expires') {
            if (-not $ignore.$field) { throw "pnpm-audit-ignores.json has an ignore without '$field'" }
        }
        if (Test-Expired "the ignore of $($ignore.id)" $ignore.expires) { "$($ignore.id) (expired $($ignore.expires))" }
    }
    if ($expired) { throw "pnpm-audit-ignores.json ignores past their expiry: $($expired -join ', '); review each and renew or drop it" }

    # pnpm audit exits non-zero whenever it finds anything, so its exit code
    # says nothing; output that is not its report is a failure of its own.
    $json = docker run --rm `
      -e COREPACK_ENABLE_DOWNLOAD_PROMPT=0 `
      -e COREPACK_HOME=/corepack `
      -v "${RepoRoot}:/app:ro" `
      -v luna-corepack:/corepack `
      -w /app `
      (Get-NodeImage) `
      bash -c 'corepack pnpm audit --json'
    $report = try { ($json -join "`n") | ConvertFrom-Json } catch { $null }
    if (-not $report -or -not $report.metadata -or $report.error) {
        throw "pnpm audit gave no report: $($json -join "`n")"
    }

    $failing = @()
    $used = @{}
    foreach ($advisory in $report.advisories.PSObject.Properties.Value) {
        $paths = ($advisory.findings | ForEach-Object { $_.paths } | Select-Object -First 3) -join ', '
        $line = "$($advisory.severity) $($advisory.github_advisory_id) $($advisory.module_name) $($advisory.vulnerable_versions) (fixed in $($advisory.patched_versions)) via $paths"
        $ignore = $ignores | Where-Object { $_.id -eq $advisory.github_advisory_id }
        if ($advisory.severity -notin 'high', 'critical') {
            Write-Host "  below the bar: $line"
        } elseif ($ignore) {
            $used[$ignore.id] = $true
            Write-Host "  ignored until $($ignore.expires): $line; $($ignore.reason)" -ForegroundColor Yellow
        } else {
            Write-Host "  FAILS: $line" -ForegroundColor Red
            $failing += $advisory.github_advisory_id
        }
    }
    foreach ($ignore in $ignores | Where-Object { -not $used[$_.id] }) {
        Write-Host "  pnpm-audit-ignores.json ignores $($ignore.id), which no longer applies: drop it" -ForegroundColor Yellow
    }
    $counts = $report.metadata.vulnerabilities
    Write-Host "  $($report.metadata.totalDependencies) packages: $($counts.critical) critical, $($counts.high) high, $($counts.moderate) moderate, $($counts.low) low"
    if ($failing) { throw "high or critical advisories: $($failing -join ', ')" }
}

# gitleaks reads the repository's object store, mounted read-only: in a
# worktree .git is a pointer to a host path, so the store is git's common dir,
# and the range's ends are resolved to commits here, where the worktree's HEAD
# is known. The default rules; --redact keeps every secret out of the output.
function Invoke-Secrets {
    $store = (git -C $RepoRoot rev-parse --path-format=absolute --git-common-dir)
    if ($LASTEXITCODE) { throw "git rev-parse failed (exit $LASTEXITCODE)" }
    $ends = foreach ($end in $Range -split '\.\.') {
        $sha = git -C $RepoRoot rev-parse --verify --quiet "$end^{commit}"
        if ($LASTEXITCODE -or -not $sha) { throw "-Range: '$end' is not a commit here" }
        $sha.Trim()
    }
    $logOpts = $ends -join '..'
    Invoke-Checked 'gitleaks' {
        docker run --rm `
          -v "$($store.Trim()):/repo.git:ro" `
          $Gitleaks `
          git /repo.git --log-opts=$logOpts --redact --verbose --no-banner | Out-Host
    }
}

$results = @()
Use-BuildLock {
    foreach ($name in $Check) {
        Write-Host "== $name" -ForegroundColor Cyan
        $timer = [Diagnostics.Stopwatch]::StartNew()
        $result = 'ok'
        try {
            switch ($name) {
                'deny' { Invoke-Deny }
                'audit' { Invoke-Audit }
                'secrets' { Invoke-Secrets }
            }
        } catch {
            $result = "FAILED: $_"
        }
        $script:results += [pscustomobject]@{
            Check  = $name
            Result = $result
            Time   = '{0:mm\:ss}' -f $timer.Elapsed
        }
    }
}

$results | Format-Table -AutoSize | Out-String -Width 200 | Write-Host
if ($results | Where-Object Result -ne 'ok') {
    Write-Host 'supply-chain checks failed' -ForegroundColor Red
    exit 1
}
Write-Host "supply-chain checks passed ($($Check -join ', '))" -ForegroundColor Green
