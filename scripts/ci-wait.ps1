#Requires -Version 7.4
# Waits for CI (.github/workflows/ci.yml) on GitHub to finish for one commit and
# reports how it ended. The gate a change passes is that run: the operator's
# mirror pushes each slice/*, gpui/*, core/* and iteration/* branch within about
# a minute of a commit, and CI runs on the push. Read-only: it never reruns,
# cancels or triggers anything, and pushes nothing.
#
#   ./scripts/ci-wait.ps1                          HEAD of this checkout, on its branch
#   ./scripts/ci-wait.ps1 -Branch slice/x -Sha b41a20a3
#
# Waits first for the run to appear (the mirror may lag), then for it to
# complete. Exit codes:
#   0  the run succeeded; prints its id, url and every job's conclusion
#   1  the run did not succeed; prints each failed job with its failing steps
#      and the tail of its log
#   2  -TimeoutMinutes passed first; says what it last saw
#   3  gh is missing or not logged in, or the repository, branch or commit could
#      not be worked out, or gh kept failing
param(
    # The commit to wait for, a full sha or a prefix of at least 7 characters;
    # HEAD of this checkout by default.
    [string]$Sha,
    # The branch it was pushed to; the checkout's current one by default.
    [string]$Branch,
    # owner/name on GitHub; taken from the origin remote by default.
    [string]$Repo,
    [int]$TimeoutMinutes = 60,
    [int]$PollSeconds = 30,
    # How many lines of each failed job's log are shown (the last ones).
    [int]$LogLines = 40
)
. "$PSScriptRoot/lib.ps1"
. "$PSScriptRoot/ci-wait-lib.ps1"

function Stop-Script([int]$Code, [string]$Message) {
    Write-Host $Message -ForegroundColor $(if ($Code -eq 3) { 'Red' } else { 'Yellow' })
    exit $Code
}

if (-not (Get-Command gh -ErrorAction SilentlyContinue)) {
    Stop-Script 3 'gh (the GitHub CLI) is not on PATH: it reads the CI runs. Install it from https://cli.github.com and run `gh auth login`.'
}
# gh's own message says which account or host is the problem.
$auth = gh auth status 2>&1
if ($LASTEXITCODE) {
    Stop-Script 3 "gh is not logged in (run 'gh auth login'):`n$($auth -join "`n")"
}

if (-not $Sha) {
    $Sha = (git -C $RepoRoot rev-parse HEAD).Trim()
    if ($LASTEXITCODE) { Stop-Script 3 'git rev-parse HEAD failed: pass -Sha' }
}
if ($Sha -notmatch '^[0-9a-fA-F]{7,40}$') { Stop-Script 3 "-Sha should be 7 to 40 hex characters, not '$Sha'" }
if (-not $Branch) {
    $Branch = (git -C $RepoRoot rev-parse --abbrev-ref HEAD).Trim()
    if ($LASTEXITCODE -or $Branch -eq 'HEAD') { Stop-Script 3 'no current branch (a detached HEAD, or no commit): pass -Branch' }
}
if (-not $Repo) {
    $origin = (git -C $RepoRoot remote get-url origin).Trim()
    if ($LASTEXITCODE) { Stop-Script 3 'no origin remote: pass -Repo owner/name' }
    $Repo = Get-RepoFromUrl $origin
    if (-not $Repo) { Stop-Script 3 "origin is not a GitHub URL ($origin): pass -Repo owner/name" }
}

# Runs gh and returns its JSON, parsed. A failed call (the network, a rate limit)
# is tried again at the next poll; a run of them ends the wait.
$ghFailures = 0
function Get-GhJson([string[]]$GhArgs) {
    $out = gh @GhArgs 2>&1
    if ($LASTEXITCODE) {
        $script:ghFailures++
        if ($script:ghFailures -ge 5) { Stop-Script 3 "gh failed five times in a row, last: gh $($GhArgs -join ' ')`n$($out -join "`n")" }
        Write-Host "gh failed (will retry): $(($out -join ' ').Trim())" -ForegroundColor DarkYellow
        return $null
    }
    $script:ghFailures = 0
    $out | ConvertFrom-Json
}

$short = $Sha.Substring(0, [Math]::Min(8, $Sha.Length))
Write-Host "Waiting for CI on $Repo, branch $Branch, commit $short (up to $TimeoutMinutes min)"
$deadline = [DateTime]::UtcNow.AddMinutes($TimeoutMinutes)
$last = 'nothing yet'
$lastPrinted = $null
$run = $null
while ($true) {
    $runs = Get-GhJson @('run', 'list', '-R', $Repo, '--branch', $Branch, '--workflow', 'ci.yml', '--limit', '50',
        '--json', 'databaseId,headSha,status,conclusion,workflowName,url')
    if ($null -ne $runs) {
        $run = Select-CiRun @($runs) $Sha
        if (-not $run) {
            $newest = @($runs | Sort-Object { [long]$_.databaseId } -Descending | Select-Object -First 1)
            $last = if ($newest) { "no CI run for $short on $Branch ($(@($runs).Count) runs there, the newest for $($newest[0].headSha.Substring(0, 8)))" } else { "no CI run on $Branch at all" }
        } else {
            $last = "run $($run.databaseId): $($run.status)"
            if ($run.status -eq 'completed') { break }
        }
        if ($last -ne $lastPrinted) {
            Write-Host "$([DateTime]::Now.ToString('HH:mm:ss'))  $last"
            $lastPrinted = $last
        }
    }
    if ([DateTime]::UtcNow.AddSeconds($PollSeconds) -gt $deadline) {
        Stop-Script 2 "Timed out after $TimeoutMinutes min. Last seen: $last. Nothing was rerun or canceled."
    }
    Start-Sleep -Seconds $PollSeconds
}

# The jobs are fetched once the run is complete, so their conclusions are final.
$detail = $null
while (-not $detail) {
    $detail = Get-GhJson @('run', 'view', "$($run.databaseId)", '-R', $Repo, '--json', 'jobs')
    if (-not $detail) { Start-Sleep -Seconds 5 }
}
Write-Host ''
Write-Host "Run $($run.databaseId): $($run.conclusion)"
Write-Host $run.url
Format-CiJobs $detail.jobs | Write-Host

if ($run.conclusion -eq 'success') { exit 0 }

Write-Host ''
Write-Host 'Failed:' -ForegroundColor Red
Format-FailedJobs $detail.jobs | Write-Host
$log = gh run view "$($run.databaseId)" -R $Repo --log-failed 2>&1
if ($LASTEXITCODE) {
    Write-Host "(the failed logs could not be read: $(($log -join ' ').Trim()); see $($run.url))"
} else {
    Write-Host ''
    Format-FailedLog ($log -join "`n") $LogLines | Write-Host
}
exit 1
