# What scripts/supply-chain.ps1 decides, as functions that
# scripts/supply-chain-lib.test.ps1 runs on inputs of its own: the ignores of
# advisories and their expiry, the findings the secrets check passes over, and
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

# The findings the secrets check passes over, from the repository's
# .gitleaksignore: one finding of the history per line, in the form gitleaks
# prints a fingerprint of a git scan, `<commit>:<file>:<rule>:<line>`, the
# commit in full. gitleaks is not handed this file (Invoke-Gitleaks compares
# its findings with the entries field by field), so the form is only how an
# entry is written, and gitleaks' commit-free `<file>:<rule>:<line>` or
# anything else is refused. A second secret the same rule finds on the same
# line of that commit is passed over with the first. Each entry sits under a
# comment of its own saying why it is no secret, and its path takes plain
# characters only, so the four fields split one way. Returns the entries as
# { Commit, File, RuleID, StartLine, Text }; none when there is no file.
function Read-GitleaksIgnore([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { return @() }
    $entries = [Collections.Generic.List[object]]::new()
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
        $entry = ConvertTo-GitleaksEntry $text
        if (-not $entry) {
            throw "$Path line ${number}: '$text' is not an exact fingerprint (<commit>:<file>:<rule>:<line>, the commit in full)"
        }
        if (-not $why -or $why.Length -lt 20 -or @($why -split '\s+').Count -lt 4) {
            throw "$Path line ${number}: $text has no comment above it saying why it is no secret"
        }
        if ($entries.Text -ccontains $text) { throw "$Path line ${number}: $text is listed twice" }
        $entries.Add($entry)
        $why = $null
    }
    , $entries.ToArray()
}

# One entry of .gitleaksignore as its four fields, or $null when it is not
# an exact fingerprint in the form Read-GitleaksIgnore takes.
function ConvertTo-GitleaksEntry([string]$Text) {
    if ($Text -cnotmatch '^(?<commit>[0-9a-f]{40}):(?<file>[A-Za-z0-9._/+-]+):(?<rule>[a-z0-9-]+):(?<line>[1-9][0-9]{0,8})\z') { return $null }
    [pscustomobject]@{ Commit = $Matches.commit; File = $Matches.file; RuleID = $Matches.rule; StartLine = [int]$Matches.line; Text = $Text }
}

# Whether the finding is the one an entry names: its commit, file, rule and
# line each equal, as gitleaks' report gives them.
function Test-GitleaksEntry($Finding, $Entry) {
    "$($Finding.Commit)" -ceq $Entry.Commit -and "$($Finding.File)" -ceq $Entry.File -and
        "$($Finding.RuleID)" -ceq $Entry.RuleID -and "$($Finding.StartLine)" -ceq "$($Entry.StartLine)"
}

# gitleaks over a repository's object store, mounted read-only, for git log's
# $LogOpts (commit ids, resolved by the caller). gitleaks reports every
# finding, with no ignore file of anyone's: it runs in the store, a .git
# folder no commit writes into, so the ignore file it looks for in its working
# folder and in the source is not there. The findings that are each exactly
# one of $Ignore's entries (Read-GitleaksIgnore) are then passed over here,
# field by field; any other fails the check. What a finding prints is its
# rule, file, line and commit, and the entry that would pass it over, never
# the matched text: gitleaks' --redact hides only each finding's own secret,
# and its verbose output and report carry the text around it, a second secret
# on the same line included. So the report stays inside the container's
# stdout, read here and not printed. Throws when a finding is left, or when
# gitleaks fails or its report does not read.
function Invoke-Gitleaks([string]$Store, [string]$LogOpts, [object[]]$Ignore = @()) {
    if ($LogOpts -notmatch '^[0-9a-f]{40}(\.\.[0-9a-f]{40})?$') { throw "gitleaks takes one commit id or a range of two, not '$LogOpts'" }
    foreach ($entry in $Ignore) {
        if (-not $entry.Text -or -not (ConvertTo-GitleaksEntry $entry.Text)) { throw "not an exact gitleaks fingerprint: '$($entry.Text)'" }
    }
    $report = docker run --rm `
      -v "${Store}:/repo.git:ro" `
      -w /repo.git `
      --entrypoint sh `
      $Gitleaks `
      -c "gitleaks git /repo.git --log-opts=$LogOpts --redact --no-banner --report-format json --report-path /tmp/report.json >&2; code=`$?; cat /tmp/report.json 2>/dev/null; exit `$code"
    $code = $LASTEXITCODE
    if ($code -notin 0, 1) { throw "gitleaks failed (exit $code)" }
    $text = ($report -join "`n").Trim()
    $findings = if ($text) {
        try { @($text | ConvertFrom-Json -ErrorAction Stop | Where-Object { $_ }) } catch { throw "gitleaks' report does not read: $($_.Exception.Message)" }
    } else { @() }
    if ($code -eq 0 -and -not $findings) { return }
    if (-not $findings) { throw 'gitleaks reported leaks but wrote no report' }
    $left = @(foreach ($finding in $findings) {
        if (-not @($Ignore | Where-Object { Test-GitleaksEntry $finding $_ })) { $finding }
    })
    $passed = $findings.Count - $left.Count
    if ($passed) { Write-Host "  passed over $passed finding(s) .gitleaksignore lists" }
    if (-not $left) { return }
    foreach ($finding in $left) {
        Write-Host "  $($finding.RuleID) in $($finding.File):$($finding.StartLine), commit $($finding.Commit) ($($finding.Commit):$($finding.File):$($finding.RuleID):$($finding.StartLine))" -ForegroundColor Red
    }
    throw "gitleaks found $($left.Count) secret(s); rotate each, then remove it from the history, or, for one that is no secret, list it in .gitleaksignore"
}
