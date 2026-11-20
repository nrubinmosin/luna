# The part of live-tests.ps1 that finds a managed CLI on disk, so that
# scripts/live-tests-lib.test.ps1 can run it on made-up trees.

# The executable the test build's managed copy of a CLI starts, or $null when
# the tree names none that is there. `$Root` is the CLI's folder
# (test-build/claude-cli, test-build/codex-cli), `$Bin` the executable's path
# inside a version folder (claude.exe, bin/codex.exe). As cli.rs reads it:
# `state.json` names the current version folder under `versions/`; a tree
# from Luna 0.19, without one yet, names it in the file `current`, which
# 0.20 reads the same way until its first change migrates the tree.
function Get-ManagedCli([string]$Root, [string]$Bin) {
    $record = Join-Path $Root 'state.json'
    $legacy = Join-Path $Root 'current'
    $dir = $null
    if (Test-Path -LiteralPath $record -PathType Leaf) {
        try {
            $dir = (Get-Content -LiteralPath $record -Raw | ConvertFrom-Json -ErrorAction Stop).current.dir
        } catch {
            Write-Warning "$record does not read ($_): the test build rebuilds it at its next start"
            return $null
        }
    } elseif (Test-Path -LiteralPath $legacy -PathType Leaf) {
        $dir = "$(Get-Content -LiteralPath $legacy -TotalCount 1)".Trim()
    }
    # A folder name and nothing else, as cli.rs `plain_name` takes one.
    if (-not $dir -or $dir -isnot [string] -or $dir -in '.', '..' -or $dir.IndexOfAny([char[]]'/\:') -ge 0) { return $null }
    $exe = Join-Path $Root 'versions' $dir $Bin
    if (Test-Path -LiteralPath $exe -PathType Leaf) { return $exe }
    $null
}
