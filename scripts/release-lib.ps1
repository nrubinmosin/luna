# What the release scripts share: release.ps1, release-bundle.ps1,
# release-verify.ps1 and sbom.ps1, and through them the release workflow
# (.github/workflows/release.yml). Each dot-sources it after lib.ps1:
#
#   . "$PSScriptRoot/lib.ps1"
#   . "$PSScriptRoot/release-lib.ps1"
#
# The functions that only compute take the repo root or their inputs as
# parameters, so release-lib.test.ps1 (run by the gate) can try them on a tree
# of its own.

# The four places a release's version is written. Each is read and written by
# the line that holds it, so a bump leaves the rest of the file, its line
# endings included, as it was: package.json and tauri.conf.json at their top
# level, Cargo.toml in its [package] table, Cargo.lock in luna's own entry.
$ReleaseVersionFiles = [ordered]@{
    'package.json'              = 'json'
    'src-tauri/tauri.conf.json' = 'json'
    'src-tauri/Cargo.toml'      = 'cargo-toml'
    'src-tauri/Cargo.lock'      = 'cargo-lock'
}

# The version line of one of those files: the match of a pattern whose groups
# are the text before the version, the version and the text after it.
function Find-VersionLine([string]$Text, [string]$Kind) {
    $pattern = switch ($Kind) {
        # The first "version" key is the top-level one in both files: the
        # build section and the plugins come after it, and neither has one.
        'json' { '(?m)^(\s*"version"\s*:\s*")([^"]*)(")' }
        # The first version key after the [package] header, before any other table.
        'cargo-toml' { '(?m)^(\[package\]\r?\n(?:(?!\[).*\n)*?version\s*=\s*")([^"]*)(")' }
        'cargo-lock' { '(?m)^(name = "luna"\r?\nversion = ")([^"]*)(")' }
    }
    [regex]::Match($Text, $pattern)
}

# Each file's version, by its path from the root.
function Get-ReleaseVersions([string]$Root = $RepoRoot) {
    $found = [ordered]@{}
    foreach ($file in $ReleaseVersionFiles.Keys) {
        $text = [IO.File]::ReadAllText((Join-Path $Root $file))
        $line = Find-VersionLine $text $ReleaseVersionFiles[$file]
        if (-not $line.Success) { throw "no version found in $file" }
        $found[$file] = $line.Groups[2].Value
    }
    $found
}

# The one version all four files agree on; throws, naming each, when they do not.
function Get-ReleaseVersion([string]$Root = $RepoRoot) {
    $found = Get-ReleaseVersions $Root
    $versions = @($found.Values | Sort-Object -Unique)
    if ($versions.Count -ne 1) {
        $each = ($found.Keys | ForEach-Object { "$_ $($found[$_])" }) -join ', '
        throw "the version files disagree: $each (./scripts/release.ps1 -Bump <version> sets all four)"
    }
    $versions[0]
}

# Semver as the updater compares it: x.y.z with an optional pre-release part.
function Assert-VersionFormat([string]$Version) {
    if ($Version -notmatch '^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$') {
        throw "'$Version' is not a version of the form 1.2.3 (or 1.2.3-rc.1)"
    }
}

# Writes $Version into all four files, each only on its version line.
function Set-ReleaseVersion([string]$Version, [string]$Root = $RepoRoot) {
    Assert-VersionFormat $Version
    # Every file is read and matched before any is written, so a file that
    # cannot be bumped leaves the other three as they were.
    $edits = foreach ($file in $ReleaseVersionFiles.Keys) {
        $path = Join-Path $Root $file
        $text = [IO.File]::ReadAllText($path)
        $line = Find-VersionLine $text $ReleaseVersionFiles[$file]
        if (-not $line.Success) { throw "no version found in $file" }
        $value = $line.Groups[1].Value + $Version + $line.Groups[3].Value
        @{ Path = $path; Text = $text.Remove($line.Index, $line.Length).Insert($line.Index, $value) }
    }
    $utf8 = [Text.UTF8Encoding]::new($false)
    foreach ($edit in $edits) { [IO.File]::WriteAllText($edit.Path, $edit.Text, $utf8) }
}

# What a run of the release workflow makes of the ref it runs on. A v* tag is
# the release of that version: signed, and drafted on GitHub unless the run is
# a dry run. Any other ref may only be a dry run, and builds unsigned: the
# signing key is kept for v* tags (the `release` environment, docs/releasing.md).
function Get-ReleasePlan([string]$Ref, [string]$Version, [bool]$DryRun) {
    if ($Ref.StartsWith('refs/tags/')) {
        $tag = $Ref.Substring('refs/tags/'.Length)
        if ($tag -cne "v$Version") {
            throw "tag $tag does not name the version the version files hold, ${Version} (tag v$Version)"
        }
        return [pscustomobject]@{ Tag = $tag; Signed = $true; Publish = -not $DryRun }
    }
    if (-not $DryRun) {
        throw "a release runs from its v* tag, not from $Ref; push the tag ./scripts/release.ps1 made, or run with dry-run"
    }
    [pscustomobject]@{ Tag = "v$Version"; Signed = $false; Publish = $false }
}

# The verdict on a commit's CI from its runs of ci.yml (the runs API's
# workflow_runs, push events only): 'green' once one has succeeded, 'pending'
# while one is still going and none has, 'red' once all are done and none
# succeeded, 'missing' while there are none.
function Get-CiVerdict([object[]]$Runs) {
    $Runs = @($Runs | Where-Object { $_ })
    if (-not $Runs) { return 'missing' }
    if ($Runs | Where-Object { $_.status -eq 'completed' -and $_.conclusion -eq 'success' }) { return 'green' }
    if ($Runs | Where-Object { $_.status -ne 'completed' }) { return 'pending' }
    'red'
}

# A tag's message as release notes: what comes before a signature block, or
# "Luna <version>" when that leaves nothing.
function Get-TagNotes([string]$Message, [string]$Version) {
    $notes = ($Message -split '(?m)^-----BEGIN ', 2)[0].Trim()
    if ($notes) { $notes } else { "Luna $Version" }
}

# Whether CHANGELOG.md has a heading for $Version: $true or $false, or $null
# when there is no CHANGELOG.md.
function Test-ChangelogHasVersion([string]$Version, [string]$Root = $RepoRoot) {
    $path = Join-Path $Root 'CHANGELOG.md'
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { return $null }
    $pattern = "(?m)^#{1,6}\s.*(?<![0-9A-Za-z.])v?$([regex]::Escape($Version))(?![0-9A-Za-z.-])"
    [regex]::IsMatch([IO.File]::ReadAllText($path), $pattern)
}

# The folder the Windows release build leaves its artifacts in
# (build-windows.ps1).
$ReleaseBuildDir = Join-Path $RepoRoot 'src-tauri/target/x86_64-pc-windows-msvc/release'

# The release build's artifacts for $Version: the exe, the installer, its
# updater signature ($null for an -Unsigned build) and the core fingerprint
# build.rs left beside the exe. Throws when one is missing.
function Find-ReleaseArtifacts([string]$Version, [switch]$Unsigned, [string]$Dir = $ReleaseBuildDir) {
    # By version, not by whatever comes first: the bundle folder keeps every
    # installer ever built there, and picking the first one alphabetically is
    # how v0.2.0 went out carrying the 0.1.0 setup and its signature, an update
    # that offered a build its own version.
    $setup = Get-ChildItem (Join-Path $Dir 'bundle/nsis') -Filter "*_${Version}_*-setup.exe" -ErrorAction SilentlyContinue |
        Select-Object -First 1
    if (-not $setup) { throw "no installer for $Version in $Dir/bundle/nsis" }
    $found = [pscustomobject]@{
        Exe         = Join-Path $Dir 'luna.exe'
        Setup       = $setup.FullName
        Signature   = if ($Unsigned) { $null } else { "$($setup.FullName).sig" }
        Fingerprint = Join-Path $Dir 'core-fingerprint.txt'
    }
    foreach ($f in @($found.Exe, $found.Setup, $found.Signature, $found.Fingerprint) | Where-Object { $_ }) {
        if (-not (Test-Path -LiteralPath $f -PathType Leaf)) { throw "missing build artifact: $f" }
    }
    $found
}

# A core fingerprint as build.rs writes it: 16 lowercase hex digits.
function Assert-CoreFingerprint([string]$Fingerprint, [string]$From) {
    if ($Fingerprint -cnotmatch '^[0-9a-f]{16}$') { throw "$From holds '$Fingerprint', not a core fingerprint" }
}

# The latest.json the updater plugin fetches: version, the core fingerprint
# (a window whose core has the same one installs the update by restarting only
# itself, updates.store.ts), the notes the update chip shows, the date, and the
# installer with its signature, which the plugin checks against the public key
# in tauri.conf.json. The installer is downloaded from the release the
# manifest is published with, in the repo the updater endpoint names: the
# endpoint, which every installed Luna polls, is the one place the repo is
# written. Returns the JSON text.
function New-UpdaterManifest {
    param(
        [Parameter(Mandatory)][string]$Version,
        [Parameter(Mandatory)][string]$CoreFingerprint,
        [Parameter(Mandatory)][AllowEmptyString()][string]$Signature,
        [Parameter(Mandatory)][string]$SetupName,
        [Parameter(Mandatory)][string]$Endpoint,
        [string]$Notes,
        [datetime]$PubDate = (Get-Date)
    )
    if ($Endpoint -notmatch '^(https://github\.com/[^/]+/[^/]+/releases)/latest/download/latest\.json$') {
        throw "the updater endpoint $Endpoint is not a GitHub latest-release latest.json"
    }
    $releases = $Matches[1]
    $manifest = [ordered]@{
        version          = $Version
        core_fingerprint = $CoreFingerprint
        notes            = if ($Notes) { $Notes } else { "Luna $Version" }
        pub_date         = $PubDate.ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ', [cultureinfo]::InvariantCulture)
        platforms        = [ordered]@{
            'windows-x86_64' = [ordered]@{
                signature = $Signature
                url       = "$releases/download/v$Version/$SetupName"
            }
        }
    }
    $manifest | ConvertTo-Json -Depth 5
}

# An SBOM as the release workflow's attestation step takes it. actions/attest
# (src/sbom.ts, checkIsCycloneDX) calls a file CycloneDX only with bomFormat,
# serialNumber and specVersion all set, and refuses anything else as
# "Unsupported SBOM format"; the check is repeated here so sbom.ps1 stops a
# release at the SBOM, not after the installer is built and signed. Throws
# naming the file and what it lacks.
function Assert-AttestableSbom([string]$Path) {
    $bom = Get-Content -LiteralPath $Path -Raw | ConvertFrom-Json -AsHashtable
    if ($bom -isnot [hashtable]) { throw "$Path is not a JSON object" }
    if ($bom.bomFormat -cne 'CycloneDX') { throw "$Path has bomFormat '$($bom.bomFormat)', not CycloneDX" }
    $missing = @('serialNumber', 'specVersion' | Where-Object { [string]::IsNullOrEmpty($bom[$_]) })
    if ($missing) { throw "$Path has no $($missing -join ', '), which actions/attest requires of a CycloneDX SBOM" }
}

# The updater endpoint in tauri.conf.json, which latest.json is published for.
function Get-UpdaterEndpoint([string]$Root = $RepoRoot) {
    $conf = Get-Content (Join-Path $Root 'src-tauri/tauri.conf.json') -Raw | ConvertFrom-Json
    $endpoints = @($conf.plugins.updater.endpoints)
    if ($endpoints.Count -ne 1) { throw "tauri.conf.json should name one updater endpoint, not $($endpoints.Count)" }
    $endpoints[0]
}
