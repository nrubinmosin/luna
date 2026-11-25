#Requires -Version 7.4
# Checks every fragment and the empty Unreleased section. On a feature branch
# it also checks the changes since the branch split from its base.
param(
    [string]$Root,
    [string]$Branch
)
. "$PSScriptRoot/lib.ps1"
. "$PSScriptRoot/release-lib.ps1"

# The app, native UI, workspace and vendored crates, installer, shipped assets,
# and Cargo's resolved dependencies live here. Dedicated tests, fixtures and
# the window's test/screenshot harness are outside the rule. Mixed production
# and test files use the None form for test-only edits. App and workspace
# manifests count for production dependencies, features and the release profile.
$productPaths = '^src-tauri/(?:(?:src|crates|vendor|windows|icons|resources)/|(?:build\.rs|updater\.json|Cargo\.lock)$)'
$testPaths = '(?:^|/)(?:tests?|fixtures|testdata|testkit|test_support)(?:/|\.rs$)|_tests?\.rs$|^src-tauri/crates/window/src/(?:(?:[^/]+/)?demo\.rs$|drive\.rs$|(?:bin/)?shots\.rs$|term/bench/)'
$manifestPaths = '^src-tauri/(?:crates/.+/)?Cargo\.toml$'
$productTables = '^(?:(?:target\..+|workspace)\.)?(?:build-)?dependencies(?:\.|$)|^(?:patch\.|features$|profile\.release(?:\.|$))'

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

function Get-ProductManifestLines([string[]]$Lines) {
    $include = $false
    foreach ($line in $Lines) {
        $line = $line.Trim()
        if (-not $line -or $line.StartsWith('#')) { continue }
        # Handles the table and subtable forms used by Cargo, including
        # quoted cfg expressions in [target.'cfg(...)'.dependencies].
        if ($line -cmatch '^\[\[?(?<table>.+?)\]\]?\s*(?:#.*)?$') {
            $include = $Matches.table -cmatch $productTables
        }
        if ($include) { $line }
    }
}

function Test-ProductManifestChange([string]$Base, [string]$Path) {
    $old = Invoke-Checked 'git ls-tree' { git -C $Root ls-tree --name-only $Base -- $Path }
    $before = if ($old) { Invoke-Checked 'git show' { git -C $Root show "$($Base):$Path" } }
    $file = Join-Path $Root $Path
    $after = if (Test-Path -LiteralPath $file -PathType Leaf) { Get-Content -LiteralPath $file }
    ((Get-ProductManifestLines $before) -join [char]10) -cne ((Get-ProductManifestLines $after) -join [char]10)
}

function Get-BranchMergeBase([string]$Branch, [string]$PullRequestBase) {
    if ($PullRequestBase) {
        return Invoke-Checked "git merge-base HEAD origin/$PullRequestBase (fetch the PR base first)" {
            git -C $Root merge-base HEAD "refs/remotes/origin/$PullRequestBase"
        }
    }

    # Exclude the branch being checked, since a push also updates its remote
    # ref. Start with master so it wins ties with integration branches.
    $refs = @(
        'refs/remotes/origin/master'
        Invoke-Checked 'git for-each-ref' {
            git -C $Root for-each-ref '--format=%(refname)' refs/remotes/origin/iteration/ refs/remotes/origin/stage6/ refs/remotes/origin/gpui/ refs/remotes/origin/core/
        }
    )
    $nearest = $null
    $distance = [long]::MaxValue
    foreach ($ref in $refs) {
        if ($ref -ceq "refs/remotes/origin/$Branch") { continue }
        $mergeBase = git -C $Root merge-base HEAD $ref
        if ($LASTEXITCODE -eq 1) { continue } # no common ancestor
        if ($LASTEXITCODE) { throw "git merge-base HEAD $ref failed (exit $LASTEXITCODE). Fetch origin and retry." }
        $commits = [long](Invoke-Checked 'git rev-list' { git -C $Root rev-list --count "$mergeBase..HEAD" })
        if ($commits -lt $distance) {
            $nearest = $mergeBase
            $distance = $commits
        }
    }
    if (-not $nearest) { throw 'No merge base with origin/master or an integration branch. Fetch origin and retry.' }
    $nearest
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
$number = '(?:0|[1-9][0-9]*)'
$prerelease = "(?:$number|[0-9]*[A-Za-z-][0-9A-Za-z-]*)"
$releaseBranch = "\Arelease(?:/v|-)$number\.$number\.$number(?:-$prerelease(?:\.$prerelease)*)?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?\z"
if ($Branch -ceq 'master' -or $Branch -cmatch $releaseBranch) { return }

$base = Get-BranchMergeBase $Branch $env:GITHUB_BASE_REF
$slug = ($Branch -csplit '/', 2)[-1]
# Include staged, unstaged and untracked work so the same check can run before
# committing. CI checks a clean checkout of the branch or its PR merge.
$changed = @(
    Get-GitPaths @('diff', '--no-renames', '--name-only', '-z', $base, '--')
    Get-GitPaths @('ls-files', '--others', '--exclude-standard', '-z')
)
foreach ($path in $changed) {
    if ($path -cmatch $testPaths) { continue }
    if ($path -cmatch $manifestPaths) {
        if (-not (Test-ProductManifestChange $base $path)) { continue }
    } elseif ($path -cnotmatch $productPaths) {
        continue
    }
    # A branch may already have brought its own fragment into this base.
    if ($fragments.Name -ccontains "$slug.md") { return }
    $previous = @(Get-GitPaths @('ls-tree', '-r', '--name-only', '-z', $base, '--', 'changes/'))
    foreach ($fragment in $fragments) {
        $name = "changes/$($fragment.Name)"
        if ($name -cin $changed -and $name -cnotin $previous) { return }
    }
    throw "Product changes ($path) require a new changes/<slug>.md changelog fragment or the branch's existing fragment. Name it after the branch without its prefix and add an entry or use ### None followed by a non-empty line explaining why no entry is needed."
}
