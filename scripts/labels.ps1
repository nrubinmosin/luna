#Requires -Version 7.4
# The repository's issue labels, kept here so they cannot drift: each run
# creates or updates every label below. The table in CONTRIBUTING.md
# ("Issues") says when each is set; the two change together.
#
#   ./scripts/labels.ps1            create or update the labels
#   ./scripts/labels.ps1 -Prune     and delete every other label (it comes off
#                                   every issue that carries it)
#   ./scripts/labels.ps1 -DryRun    print what would change, change nothing
#
# Bug, Feature and Task are issue types, set by the issue forms, not labels;
# GitHub's own close reasons stand in for duplicate, invalid and wontfix.
# Needs gh, logged in with rights to the repository.
param(
    [string]$Repo = 'nrubinmosin/luna',
    [switch]$Prune,
    [switch]$DryRun
)
$ErrorActionPreference = 'Stop'

$labels = @(
    @{ Name = 'area:window'; Color = '1d76db'; Description = 'The native window: panes, boards, chats, settings' }
    @{ Name = 'area:sessions'; Color = '0e8a16'; Description = 'Launching the CLIs, terminals, status, worktrees, the library' }
    @{ Name = 'area:agents'; Color = '5319e7'; Description = 'Helpers and leads, MCP tools, notices' }
    @{ Name = 'area:accounts'; Color = 'fbca04'; Description = 'Accounts, usage limits, the managed CLI copies' }
    @{ Name = 'area:core'; Color = 'b60205'; Description = 'The core process, power, updates and the installer' }
    @{ Name = 'area:repo'; Color = 'c5def5'; Description = 'CI, scripts, docs and skills for contributors' }
    @{ Name = 'cli:claude'; Color = 'd97757'; Description = 'Only Claude Code has it' }
    @{ Name = 'cli:codex'; Color = '10a37f'; Description = 'Only Codex has it' }
    @{ Name = 'needs-info'; Color = 'd4c5f9'; Description = 'Waiting on the reporter for details' }
    @{ Name = 'blocked'; Color = 'e99695'; Description = 'Waiting on another issue or an outside change, named in a comment' }
    @{ Name = 'good first issue'; Color = '7057ff'; Description = 'Small and self-contained, a fit for a first contribution' }
    @{ Name = 'help wanted'; Color = '008672'; Description = 'The maintainer would welcome a pull request' }
)

function Invoke-Gh([string[]]$GhArgs) {
    if ($DryRun) {
        Write-Host "gh $($GhArgs -join ' ')"
        return
    }
    & gh @GhArgs
    if ($LASTEXITCODE) { throw "gh $($GhArgs[0..1] -join ' ') failed (exit $LASTEXITCODE)" }
}

$existing = gh label list --repo $Repo --limit 500 --json name | ConvertFrom-Json | ForEach-Object name
if ($LASTEXITCODE) { throw "gh label list failed (exit $LASTEXITCODE)" }

foreach ($label in $labels) {
    # --force updates a label that is already there, so a rerun changes only
    # what differs.
    Invoke-Gh @('label', 'create', $label.Name, '--repo', $Repo, '--color', $label.Color,
        '--description', $label.Description, '--force')
}

if ($Prune) {
    $keep = $labels.Name
    foreach ($name in $existing | Where-Object { $_ -notin $keep }) {
        Invoke-Gh @('label', 'delete', $name, '--repo', $Repo, '--yes')
    }
}
