#Requires -Version 7.4
# Tests for ci-wait-lib.ps1, on sample JSON and logs as `gh` prints them.
# Run: pwsh scripts/ci-wait-lib.test.ps1 (the gate runs it with the scripts group)
. "$PSScriptRoot/lib.ps1"
. "$PSScriptRoot/ci-wait-lib.ps1"

$failures = 0
function Check([string]$Name, [scriptblock]$Test) {
    try {
        & $Test
        Write-Host "ok   $Name"
    } catch {
        $script:failures++
        Write-Host "FAIL ${Name}: $_" -ForegroundColor Red
    }
}
function Assert-Equal($Actual, $Expected) {
    if ($Actual -cne $Expected) { throw "expected '$Expected', got '$Actual'" }
}
function Assert-Throws([scriptblock]$Block, [string]$Like) {
    try { & $Block } catch {
        if ("$_" -notlike "*$Like*") { throw "threw '$_', not *$Like*" }
        return
    }
    throw 'did not throw'
}

# GitHub's own spelling of a run or job that was stopped.
# lang-check: allow
$stopped = 'cancelled'

$runs = @"
[
  {"databaseId":300,"headSha":"bbbbbbb1111111111111111111111111111111aa","status":"in_progress","conclusion":"","workflowName":"CI","url":"u300"},
  {"databaseId":250,"headSha":"aaaaaaa2222222222222222222222222222222bb","status":"completed","conclusion":"failure","workflowName":"CI","url":"u250"},
  {"databaseId":200,"headSha":"aaaaaaa2222222222222222222222222222222bb","status":"completed","conclusion":"success","workflowName":"Security","url":"u200"},
  {"databaseId":100,"headSha":"aaaaaaa2222222222222222222222222222222bb","status":"completed","conclusion":"$stopped","workflowName":"CI","url":"u100"}
]
"@ | ConvertFrom-Json

Check 'the run is picked by its commit, not by being the newest' {
    Assert-Equal (Select-CiRun $runs 'aaaaaaa2222222222222222222222222222222bb').databaseId 250
    Assert-Equal (Select-CiRun $runs 'bbbbbbb1').databaseId 300
}
Check 'a prefix of the sha matches, in any case' {
    Assert-Equal (Select-CiRun $runs 'AAAAAAA2').databaseId 250
}
Check 'of several runs for one commit the newest wins, another workflow ignored' {
    Assert-Equal (Select-CiRun $runs 'aaaaaaa').databaseId 250
    Assert-Equal (Select-CiRun $runs 'aaaaaaa' 'Security').databaseId 200
}
Check 'no run for a commit gives nothing' {
    Assert-Equal ($null -eq (Select-CiRun $runs 'ccccccc')) $true
    Assert-Equal ($null -eq (Select-CiRun @() 'ccccccc')) $true
}
Check 'a sha too short to tell commits apart is refused' {
    Assert-Throws { Select-CiRun $runs 'aaa' } 'at least 7'
}

Check 'the repository comes out of an origin URL in each form' {
    Assert-Equal (Get-RepoFromUrl 'https://github.com/nrubinmosin/luna.git') 'nrubinmosin/luna'
    Assert-Equal (Get-RepoFromUrl 'https://github.com/nrubinmosin/luna') 'nrubinmosin/luna'
    Assert-Equal (Get-RepoFromUrl 'https://user@github.com/o/r.git') 'o/r'
    Assert-Equal (Get-RepoFromUrl 'git@github.com:o/r.git') 'o/r'
    Assert-Equal (Get-RepoFromUrl 'ssh://git@github.com/o/r.git') 'o/r'
    Assert-Equal ($null -eq (Get-RepoFromUrl 'https://example.com/o/r.git')) $true
    Assert-Equal ($null -eq (Get-RepoFromUrl 'E:\some\path')) $true
}

$jobs = @"
[
  {"name":"frontend","status":"completed","conclusion":"success","steps":[
    {"number":1,"name":"Set up job","conclusion":"success"}]},
  {"name":"rust-linux","status":"completed","conclusion":"failure","steps":[
    {"number":1,"name":"Set up job","conclusion":"success"},
    {"number":3,"name":"Run ./scripts/gate.ps1 -Group rust","conclusion":"failure"},
    {"number":4,"name":"Post Run actions/checkout","conclusion":"skipped"}]},
  {"name":"windows-build","status":"completed","conclusion":"$stopped","steps":[]},
  {"name":"windows-tests","status":"completed","conclusion":"skipped","steps":[]},
  {"name":"extra","status":"in_progress","conclusion":"","steps":[]}
]
"@ | ConvertFrom-Json

Check 'jobs are listed with their conclusion, or their status while running' {
    $lines = @(Format-CiJobs $jobs)
    Assert-Equal $lines.Count 5
    Assert-Equal $lines[0].Trim() 'success        frontend'
    Assert-Equal $lines[4].Trim() 'in_progress    extra'
}
Check 'failed jobs come with their failing steps, skipped ones left out' {
    $lines = @(Format-FailedJobs $jobs)
    Assert-Equal ($lines -join '|') "  rust-linux: failure|    step 3: Run ./scripts/gate.ps1 -Group rust (failure)|  windows-build: $stopped"
}
Check 'no failures give no lines' {
    Assert-Equal @(Format-FailedJobs @($jobs[0], $jobs[3])).Count 0
}

$esc = [char]27
$log = (1..5 | ForEach-Object { "rust-linux`tRun gate`t2026-11-19T17:13:1$($_).0000000Z line $_" }) -join "`r`n"
$log += "`r`nrust-linux`tRun gate`t2026-11-19T17:13:20.0000000Z ${esc}[31merror${esc}[0m: boom`r`n"
$log += "windows-build`tBuild`t2026-11-19T17:13:21.0000000Z only line`r`n"

Check 'a failed log is cut to the last lines of each job, stamps and colors dropped' {
    $lines = @(Format-FailedLog $log 2)
    Assert-Equal ($lines -join '|') '--- rust-linux / Run gate|    (4 earlier lines left out)|line 5|error: boom|--- windows-build / Build|only line'
}
Check 'a log with room for all of it is shown whole' {
    $lines = @(Format-FailedLog $log 40)
    Assert-Equal $lines.Count 9
    Assert-Equal $lines[1] 'line 1'
}
Check 'an empty log gives nothing' {
    Assert-Equal @(Format-FailedLog '').Count 0
}

if ($failures) { Write-Host "$failures failed" -ForegroundColor Red; exit 1 }
Write-Host 'all passed'
