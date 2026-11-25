#Requires -Version 7.4
# Checks every fragment and the empty Unreleased section. On a feature branch
# it also checks the changes since the branch split from origin/master.
param(
    [string]$Root,
    [string]$Branch
)
. "$PSScriptRoot/lib.ps1"
. "$PSScriptRoot/release-lib.ps1"

# The app, native UI and other workspace crates, installer, and shipped assets
# live here. Dedicated tests, fixtures and the window's test/screenshot harness
# are outside the rule. Mixed production and test files use the None form for
# test-only edits. Cargo manifests count only for shipped dependency tables.
$productPaths = '^src-tauri/(?:(?:src|crates|windows|icons|resources)/|(?:build\.rs|updater\.json)$)'
$testPaths = '(?:^|/)(?:tests?|fixtures|testdata|testkit|test_support)(?:/|\.rs$)|_tests?\.rs$|^src-tauri/crates/window/src/(?:(?:[^/]+/)?demo\.rs$|drive\.rs$|(?:bin/)?shots\.rs$|term/bench/)'
$manifestPaths = '^src-tauri/(?:crates/.+/)?Cargo\.toml$'
$dependencyTables = '^(?:(?:target\..+|workspace)\.)?(?:build-)?dependencies(?:\.|$)|^patch\.'

function Get-GitPaths([string[]]$GitArgs) {
    # NUL-delimited paths keep the actual file names, including Git's usual
    # quoting characters, without making them part of a shell command.
    $start = [Diagnostics.ProcessStartInfo]::new('git')
    $start.WorkingDirectory = $Root
    $start.UseShellExecute = $false
    $start.RedirectStandardOutput = $true
    $start.StandardOutputEncoding = [Text.Encoding]::UTF8
    foreach ($arg in $GitArgs) { $start.ArgumentList.Add($arg) }
    $process = [Diagnostics.Process]::Start($start)
    try {
        $paths = $process.StandardOutput.ReadToEnd()
        $process.WaitForExit()
        if ($process.ExitCode) { throw "git $($GitArgs[0]) failed (exit $($process.ExitCode))" }
        $paths.Split([char]0, [StringSplitOptions]::RemoveEmptyEntries)
    } finally {
        $process.Dispose()
    }
}

function Get-DependencyLines([string[]]$Lines) {
    $include = $false
    foreach ($line in $Lines) {
        $line = $line.Trim()
        if (-not $line -or $line.StartsWith('#')) { continue }
        # Handles the table and subtable forms used by Cargo, including
        # quoted cfg expressions in [target.'cfg(...)'.dependencies].
        if ($line -cmatch '^\[\[?(?<table>.+?)\]\]?\s*(?:#.*)?$') {
            $include = $Matches.table -cmatch $dependencyTables
        }
        if ($include) { $line }
    }
}

function Test-DependencyChange([string]$Base, [string]$Path) {
    $old = Invoke-Checked 'git ls-tree' { git -C $Root ls-tree --name-only $Base -- $Path }
    $before = if ($old) { Invoke-Checked 'git show' { git -C $Root show "$($Base):$Path" } }
    $file = Join-Path $Root $Path
    $after = if (Test-Path -LiteralPath $file -PathType Leaf) { Get-Content -LiteralPath $file }
    ((Get-DependencyLines $before) -join [char]10) -cne ((Get-DependencyLines $after) -join [char]10)
}

if (-not $Root) { $Root = $RepoRoot }
$Root = (Resolve-Path -LiteralPath $Root).ProviderPath
$fragments = @(Get-ChangeFragments $Root)
Assert-UnreleasedEmpty (Read-Changelog $Root)

if (-not $Branch -and $env:GITHUB_ACTIONS -ceq 'true') {
    $Branch = if ($env:GITHUB_HEAD_REF) { $env:GITHUB_HEAD_REF } else { $env:GITHUB_REF_NAME }
}
if (-not $Branch) {
    $Branch = Invoke-Checked 'git branch' { git -C $Root branch --show-current }
}
if ($Branch -cmatch '^(?:master$|release(?:$|/|-))') { return }

$base = Invoke-Checked 'git merge-base HEAD origin/master (fetch origin/master first)' {
    git -C $Root merge-base HEAD origin/master
}
# Include staged, unstaged and untracked work so the same check can run before
# committing. CI checks a clean checkout of the branch or its PR merge.
$changed = @(
    Get-GitPaths @('diff', '--no-renames', '--name-only', '-z', $base, '--')
    Get-GitPaths @('ls-files', '--others', '--exclude-standard', '-z')
)
foreach ($path in $changed) {
    if ($path -cmatch $testPaths) { continue }
    if ($path -cmatch $manifestPaths) {
        if (-not (Test-DependencyChange $base $path)) { continue }
    } elseif ($path -cnotmatch $productPaths) {
        continue
    }
    $previous = @(Get-GitPaths @('ls-tree', '-r', '--name-only', '-z', $base, '--', 'changes/'))
    foreach ($fragment in $fragments) {
        $name = "changes/$($fragment.Name)"
        if ($name -cin $changed -and $name -cnotin $previous) { return }
    }
    throw "Product changes ($path) require a new changes/<slug>.md changelog fragment. Add an entry or use ### None followed by a non-empty line explaining why no entry is needed."
}
