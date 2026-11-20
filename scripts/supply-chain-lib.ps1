# What scripts/supply-chain.ps1 decides, as functions that
# scripts/supply-chain-lib.test.ps1 runs on inputs of its own: the ignores of
# advisories and their expiry, the findings gitleaks is told to pass over, and
# gitleaks' findings without their secrets.
# Dot-source it after lib.ps1.

# cargo-deny's release binary, checked against the SHA-256 pinned here before
# it runs: built from its own lockfile by its maintainers, where a `cargo
# install` here would compile it and its whole tree on every CI run.
$CargoDeny = '0.20.2'
$CargoDenySha256 = '9f12ed4c49936e09b48bf862b595cde2fe64fcbd9d74dfacac6131ca824c8d5f'
# gitleaks' official image, by tag for people and digest for the image itself.
$Gitleaks = 'ghcr.io/gitleaks/gitleaks:v8.30.1@sha256:c00b6bd0aeb3071cbcb79009cb16a60dd9e0a7c60e2be9ab65d25e6bc8abbb7f'

# The Rust image the Windows toolchain image builds on, by the same tag and
# digest, as scripts/sbom.ps1 uses it: cargo-deny fetches the advisory database
# with the git CLI, which the slim image lacks, and its Python reads TOML.
function Get-RustImage {
    $rust = Get-BaseImages | Where-Object { $_.File -eq 'docker/windows-build.Dockerfile' -and $_.Repo -eq 'rust' }
    if (@($rust).Count -ne 1) { throw 'docker/windows-build.Dockerfile should have one FROM rust: line' }
    $rust
}

# An ignore's reason, checked: it says something, and ends on its expiry.
# Returns the expiry date. A reason of a few words cannot say who pulls the
# package in and why it cannot go, so it is refused, as is a blank one.
function Get-IgnoreExpiry([string]$What, $Reason) {
    $text = if ($Reason -is [string]) { $Reason.Trim() } else { '' }
    if (-not $text) { throw "$What has no reason worth the name (none given): say who pulls the package in and why it cannot go yet" }
    if ($text -notmatch '\s*Expires (\S+?)\.?$') { throw "$What has no 'Expires YYYY-MM-DD' at the end of its reason" }
    $expires = $Matches[1]
    $text = $text.Substring(0, $text.Length - $Matches[0].Length).Trim()
    if ($text.Length -lt 20 -or @($text -split '\s+').Count -lt 4) {
        throw "$What has no reason worth the name ('$text'): say who pulls the package in and why it cannot go yet"
    }
    $date = [datetime]::MinValue
    if (-not [datetime]::TryParseExact($expires.Trim(), 'yyyy-MM-dd', [cultureinfo]::InvariantCulture, 'None', [ref]$date)) {
        throw "$What has an expiry that is not a yyyy-MM-dd date: '$expires'"
    }
    $date
}

# deny.toml's [advisories] ignore, read by a TOML parser (Python's tomllib, in
# the Rust image), so that every form cargo-deny accepts is seen: a bare id, an
# inline table however it is laid out, an array of tables. Each comes back as
# { Id, Reason }, Reason $null where the entry has none.
function Get-DenyIgnores([string]$DenyToml) {
    $file = (Resolve-Path -LiteralPath $DenyToml).Path
    $json = docker run --rm `
      -v "$(Split-Path $file -Parent):/policy:ro" `
      (Get-RustImage).Ref `
      python3 -c "import json, sys, tomllib; print(json.dumps(tomllib.load(open(sys.argv[1], 'rb')).get('advisories', {}).get('ignore', [])))" "/policy/$(Split-Path $file -Leaf)"
    if ($LASTEXITCODE) { throw "$DenyToml does not parse as TOML (exit $LASTEXITCODE)" }
    foreach ($entry in (($json -join "`n") | ConvertFrom-Json)) {
        if ($entry -is [string]) {
            [pscustomobject]@{ Id = $entry; Reason = $null }
        } else {
            [pscustomobject]@{ Id = $entry.id; Reason = $entry.reason }
        }
    }
}

# Throws unless every ignore has an id, a reason worth the name and an expiry
# not yet past.
function Assert-DenyIgnores([object[]]$Entries, [datetime]$Today) {
    $expired = foreach ($entry in $Entries) {
        $id = if ($entry.Id -is [string] -and $entry.Id.Trim()) { $entry.Id } else { throw "deny.toml has an ignore with no id" }
        $date = Get-IgnoreExpiry "deny.toml's ignore of $id" $entry.Reason
        if ($date -lt $Today) { "$id (expired $($date.ToString('yyyy-MM-dd')))" }
    }
    if ($expired) { throw "src-tauri/deny.toml ignores past their expiry: $($expired -join ', '); review each and renew or drop it" }
}

# The findings gitleaks passes over, from the repository's .gitleaksignore,
# the file gitleaks itself reads: one exact fingerprint of a finding in the
# history per line, as gitleaks forms it for a git scan,
# `<commit>:<file>:<rule>:<line>`, so an entry covers that one finding and
# nothing a later commit adds. gitleaks would also take `<file>:<rule>:<line>`,
# which holds for every commit, and anything else it skips without a word; both
# are refused here. Each entry sits under a comment of its own saying why it is
# no secret, and a path takes plain characters only, as the fingerprints go
# into gitleaks' container as text. Returns the fingerprints; none when there
# is no file.
function Read-GitleaksIgnore([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { return @() }
    $fingerprints = [Collections.Generic.List[string]]::new()
    $why = $null
    $number = 0
    foreach ($line in (Get-Content -LiteralPath $Path)) {
        $number++
        $text = $line.Trim()
        if (-not $text) { $why = $null; continue }
        if ($text.StartsWith('#')) {
            $said = $text.TrimStart('#').Trim()
            $why = if ($why) { "$why $said" } else { $said }
            continue
        }
        if ($text -cnotmatch '^[0-9a-f]{40}:[A-Za-z0-9._/+-]+:[a-z0-9-]+:[1-9][0-9]*$') {
            throw "$Path line ${number}: '$text' is not an exact fingerprint (<commit>:<file>:<rule>:<line>, the commit in full)"
        }
        if (-not $why -or $why.Length -lt 20 -or @($why -split '\s+').Count -lt 4) {
            throw "$Path line ${number}: $text has no comment above it saying why it is no secret"
        }
        if ($fingerprints.Contains($text)) { throw "$Path line ${number}: $text is listed twice" }
        $fingerprints.Add($text)
        $why = $null
    }
    , $fingerprints.ToArray()
}

# gitleaks over a repository's object store, mounted read-only, for git log's
# $LogOpts (commit ids, resolved by the caller), passing over the findings
# $Ignore names (Read-GitleaksIgnore). What a finding prints is its rule,
# file, line and commit, and the fingerprint an ignore would take, never the
# matched text: gitleaks' --redact hides only each finding's own secret, and
# its verbose output and report carry the text around it, a second secret on
# the same line included. So the report stays inside the container's stdout,
# read here and not printed. Throws when there are findings, or when gitleaks
# fails.
function Invoke-Gitleaks([string]$Store, [string]$LogOpts, [string[]]$Ignore = @()) {
    if ($LogOpts -notmatch '^[0-9a-f]{40}(\.\.[0-9a-f]{40})?$') { throw "gitleaks takes one commit id or a range of two, not '$LogOpts'" }
    # Held to Read-GitleaksIgnore's form again, as each goes into the shell
    # line below between single quotes.
    foreach ($fingerprint in $Ignore) {
        if ($fingerprint -cnotmatch '^[0-9a-f]{40}:[A-Za-z0-9._/+-]+:[a-z0-9-]+:[1-9][0-9]*$') { throw "not an exact gitleaks fingerprint: '$fingerprint'" }
    }
    $listed = ($Ignore | ForEach-Object { "'$_'" }) -join ' '
    $report = docker run --rm `
      -v "${Store}:/repo.git:ro" `
      --entrypoint sh `
      $Gitleaks `
      -c "printf '%s\n' $listed > /tmp/.gitleaksignore; gitleaks git /repo.git --log-opts=$LogOpts --gitleaks-ignore-path /tmp/.gitleaksignore --redact --no-banner --report-format json --report-path /tmp/report.json >&2; code=`$?; cat /tmp/report.json 2>/dev/null; exit `$code"
    $code = $LASTEXITCODE
    if ($code -notin 0, 1) { throw "gitleaks failed (exit $code)" }
    $text = ($report -join "`n").Trim()
    $findings = if ($text) { @($text | ConvertFrom-Json | Where-Object { $_ }) } else { @() }
    if ($code -eq 0 -and -not $findings) { return }
    if (-not $findings) { throw 'gitleaks reported leaks but wrote no report' }
    foreach ($finding in $findings) {
        Write-Host "  $($finding.RuleID) in $($finding.File):$($finding.StartLine), commit $($finding.Commit) ($($finding.Commit):$($finding.File):$($finding.RuleID):$($finding.StartLine))" -ForegroundColor Red
    }
    throw "gitleaks found $($findings.Count) secret(s); rotate each, then remove it from the history, or, for one that is no secret, list its fingerprint in .gitleaksignore"
}
