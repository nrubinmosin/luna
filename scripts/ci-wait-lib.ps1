# The parts of ci-wait.ps1 that decide rather than talk to GitHub, so that
# scripts/ci-wait-lib.test.ps1 can run them on sample JSON from `gh`.

# "owner/name" from a remote URL (https or ssh form, with or without .git), or
# $null when it is not a GitHub one.
function Get-RepoFromUrl([string]$Url) {
    if ($Url -match '^(?:https?://(?:[^/@]+@)?github\.com/|(?:ssh://)?git@github\.com[:/])(?<repo>[^/\s]+/[^/\s]+?)(?:\.git)?/?$') {
        return $Matches.repo
    }
    $null
}

# The ci.yml run for a commit among what `gh run list` returned for a branch:
# one whose head is $Sha (a full sha, or a prefix of at least 7 characters).
# Several runs can share a commit (a rerun, a push and a pull request): the
# newest, by id, is the one that counts. $null when there is none yet.
function Select-CiRun($Runs, [string]$Sha, [string]$Workflow = 'CI') {
    if ($Sha.Length -lt 7) { throw "a sha of at least 7 characters is needed, not '$Sha'" }
    $Runs |
        Where-Object { $_.workflowName -eq $Workflow -and $_.headSha -and $_.headSha.StartsWith($Sha, [StringComparison]::OrdinalIgnoreCase) } |
        Sort-Object { [long]$_.databaseId } -Descending |
        Select-Object -First 1
}

# One line per job of a run (`gh run view --json jobs`): its name and what it
# ended in, or where it still is.
function Format-CiJobs($Jobs) {
    foreach ($job in $Jobs) {
        $state = if ($job.conclusion) { $job.conclusion } else { $job.status }
        '  {0,-14} {1}' -f $state, $job.name
    }
}

# What failed in a run: each job that did not succeed with the steps that did
# not, as lines. Skipped jobs and steps are not failures; a job that failed
# without a failing step (a runner lost, a timeout) is listed as it is.
function Format-FailedJobs($Jobs) {
    $bad = { param($Conclusion) $Conclusion -and $Conclusion -notin @('success', 'skipped', 'neutral') }
    foreach ($job in $Jobs | Where-Object { & $bad $_.conclusion }) {
        "  $($job.name): $($job.conclusion)"
        foreach ($step in $job.steps | Where-Object { & $bad $_.conclusion }) {
            "    step $($step.number): $($step.name) ($($step.conclusion))"
        }
    }
}

# `gh run view --log-failed`'s lines ("job<TAB>step<TAB>timestamp text") cut to
# the last $Lines of each job, which is where a failure is reported, under a
# heading of the job and step; the timestamp and any color codes are dropped.
function Format-FailedLog([string]$Log, [int]$Lines = 40) {
    $esc = [char]27
    $groups = [ordered]@{}
    foreach ($line in $Log -split "\r?\n") {
        $parts = $line -split "`t", 3
        if ($parts.Count -lt 3) { continue }
        $key = "$($parts[0]) / $($parts[1])"
        if (-not $groups.Contains($key)) { $groups[$key] = [System.Collections.Generic.List[string]]::new() }
        $text = ($parts[2] -replace '^\S+Z ?', '') -replace "$esc\[[0-9;]*[A-Za-z]", ''
        $groups[$key].Add($text)
    }
    foreach ($key in $groups.Keys) {
        $all = $groups[$key]
        "--- $key"
        if ($all.Count -gt $Lines) { "    ($($all.Count - $Lines) earlier lines left out)" }
        $all | Select-Object -Last $Lines
    }
}
