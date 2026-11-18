#Requires -Version 7.4
# Idle cost of the spike: starts spike.exe off screen and never activated
# (--background plus -AppArgs), waits -Warm seconds, then samples its own
# process tree (by its PID, children too) every -Every seconds for -Seconds:
# CPU as % of one core (TotalProcessorTime deltas), private bytes, working set,
# handles and threads per process, the spike's busiest threads (name, start
# module), the app's render counters (its --frames file), and the foreground
# window before, at every sample and after, which must never change and never
# be the spike's (a change the operator makes is logged, not failed). Writes results/<Name>-idle.json and prints a summary line.
# Takes no input, activates nothing, and kills only the process it started.
#
#   pwsh ./measure-idle.ps1 -Name bare -AppArgs '--bare'
#   pwsh ./measure-idle.ps1 -Name editor -AppArgs '--editor,samples/sample.md' -Seconds 60
param(
    [string]$Name = 'idle',
    [string[]]$AppArgs = @(),
    [double]$Warm = 10,
    [double]$Seconds = 60,
    [double]$Every = 5,
    [int]$TopThreads = 5,
    [string]$Exe = 'spike.exe',
    # Leave --frames out: the app then writes no counters (a 1 Hz wake of its own).
    [switch]$NoFrames
)
$ErrorActionPreference = 'Stop'
# Through `pwsh -File` an array arrives as one comma-joined string.
$AppArgs = @($AppArgs | ForEach-Object { $_ -split ',' } | Where-Object { $_ })
$spike = $PSScriptRoot
. (Join-Path $spike 'sampler.ps1')
$results = Join-Path $spike 'results'
New-Item -ItemType Directory -Force $results | Out-Null
$frames = Join-Path $results "$Name-frames.json"
$jsonPath = Join-Path $results "$Name-idle.json"
Remove-Item $frames -ErrorAction Ignore

$fg0 = [SpikeSampler]::GetForegroundWindow()
$runFor = [math]::Ceiling($Warm + $Seconds + 30)
$runArgs = @('--background', '--exit-after', "$runFor") + $(if ($NoFrames) { @() } else { @('--frames', $frames) }) + $AppArgs
$null = Get-CyclesPerSecond
Start-ForegroundWatch
$proc = Start-Spike (Join-Path $spike $Exe) $runArgs $spike
$samples = [Collections.Generic.List[object]]::new()
$fgChanges = 0
$fgSpike = 0
try {
    Start-Sleep -Seconds $Warm
    if ($proc.HasExited) { throw "spike exited during warm-up (code $($proc.ExitCode))" }
    $windows = Assert-OffScreen $proc.Id 'measure-idle:'
    $prev = Get-TreeSnapshot $proc $frames
    $first = $prev
    $t0 = [Diagnostics.Stopwatch]::StartNew()
    for ($e = $Every; $e -le $Seconds + 0.001; $e += $Every) {
        Start-Sleep -Milliseconds ([math]::Max(0, $e * 1000 - $t0.ElapsedMilliseconds))
        $now = Get-TreeSnapshot $proc $frames
        $row = Compare-Snapshot $prev $now $proc.Id $TopThreads $fg0
        if (-not $row.foregroundUnchanged) { $fgChanges++ }
        if ($row.foregroundIsSpike) { $fgSpike++ }
        $samples.Add($row)
        Write-Host ("t={0,5:n0}s spike {1,5:n2}% cycles {7,5:n2}% (tree {2,5:n2}%) private {3:n1} MB renders main {4} editor {5} fg ok {6}" -f
            $e, $row.spikeCpuPct, $row.treeCpuPct, $row.spikePrivateMB, $row.mainRenders, $row.editorRenders, ($row.foregroundUnchanged -and -not $row.foregroundIsSpike), $row.spikeCyclePct)
        $prev = $now
    }
    $total = Compare-Snapshot $first $prev $proc.Id $TopThreads $fg0
} finally {
    if (-not $proc.HasExited) { $proc.Kill($true) }
    $proc.WaitForExit(10000) | Out-Null
    $changes = Stop-ForegroundWatch
}
$spikeFront = Get-SpikeForeground $changes @($proc.Id)
$fg1 = [SpikeSampler]::GetForegroundWindow()
$summary = [ordered]@{
    name = $Name
    args = $runArgs -join ' '
    seconds = $total.seconds
    spikeCpuPct = $total.spikeCpuPct
    spikeCpuPctMaxSample = ($samples | Measure-Object spikeCpuPct -Maximum).Maximum
    spikeCyclePct = $total.spikeCyclePct
    spikeCyclePctMaxSample = ($samples | Measure-Object spikeCyclePct -Maximum).Maximum
    cyclesPerSecond = [math]::Round((Get-CyclesPerSecond) / 1e6, 1)
    windows = $windows
    treeCpuPct = $total.treeCpuPct
    treeCyclePct = $total.treeCyclePct
    spikePrivateMB = $total.spikePrivateMB
    spikeWorkingSetMB = $total.spikeWorkingSetMB
    spikeHandles = $total.spikeHandles
    spikeThreads = $total.spikeThreads
    mainRendersPerSec = $total.mainRendersPerSec
    editorRendersPerSec = $total.editorRendersPerSec
    topThreads = $total.topThreads
    processes = $total.processes
    foregroundBefore = Get-WindowLabel $fg0 $proc.Id
    foregroundAfter = Get-WindowLabel $fg1 $proc.Id
    # Every foreground change during the run (SetWinEventHook), the operator's among them.
    foregroundChanges = $changes
    # Changes the operator makes are logged; only the spike in front fails the run.
    foregroundChangedSamples = $fgChanges
    foregroundUnchanged = ($fg0 -eq $fg1) -and $fgChanges -eq 0
    foregroundSpikeSamples = $fgSpike
    foregroundWasSpike = $fgSpike -gt 0 -or $spikeFront.Count -gt 0 -or [SpikeSampler]::Pid($fg1) -eq $proc.Id
    exited = $proc.HasExited
}
[pscustomobject]@{ summary = $summary; samples = $samples } | ConvertTo-Json -Depth 6 | Set-Content $jsonPath
$top = ($total.topThreads | Select-Object -First 3 | ForEach-Object { "$($_.name)@$($_.module) $($_.cpuPct)%/$($_.cyclePct)%" }) -join ', '
Write-Host ("{0}: spike {1:n2}% of one core (cycles {12:n2}%) over {2:n0} s (max sample {3:n2}%), tree {4:n2}%, private {5:n1} MB, renders/s main {6} editor {7}, top threads: {8}; foreground unchanged: {9}, ever the spike: {11}; json {10}" -f
    $Name, $summary.spikeCpuPct, $summary.seconds, $summary.spikeCpuPctMaxSample, $summary.treeCpuPct, $summary.spikePrivateMB,
    $summary.mainRendersPerSec, $summary.editorRendersPerSec, $top, $summary.foregroundUnchanged, $jsonPath, $summary.foregroundWasSpike, $summary.spikeCyclePct)
if ($summary.foregroundWasSpike) { throw 'the spike came to the foreground' }
