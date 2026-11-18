#Requires -Version 7.4
# A long idle run of the spike: starts spike.exe off screen and never
# activated (--background plus -AppArgs) and samples its own process tree
# every -Every seconds for -Hours, with the same sampler as measure-idle.ps1
# (sampler.ps1). Each sample is appended to results/<Name>-soak.jsonl as it is
# taken, so a run cut short is still readable. At the end (or when stopped):
# private bytes after the warm-up (min, max, delta), the spike's CPU mean and
# p95 after it, render rates, and foreground changes (the operator's are
# logged; the spike ever in front fails the run), in results/<Name>-soak.json.
# Takes no input, activates nothing, and kills only the process it started.
#
#   pwsh ./soak.ps1 -Name soak -Hours 4 -Every 60 -AppArgs '--term,--pty,--editor,samples/sample.md'
#   pwsh ./soak.ps1 -Name dry -Hours 0.05 -Every 10 -WarmMinutes 1 -AppArgs ...   (a 3-minute dry run)
param(
    [string]$Name = 'soak',
    [string[]]$AppArgs = @(),
    [double]$Hours = 4,
    [double]$Every = 60,
    [double]$WarmMinutes = 10,
    [int]$TopThreads = 3,
    [string]$Exe = 'spike.exe'
)
$ErrorActionPreference = 'Stop'
# Through `pwsh -File` an array arrives as one comma-joined string.
$AppArgs = @($AppArgs | ForEach-Object { $_ -split ',' } | Where-Object { $_ })
$spike = $PSScriptRoot
. (Join-Path $spike 'sampler.ps1')
$results = Join-Path $spike 'results'
New-Item -ItemType Directory -Force $results | Out-Null
$frames = Join-Path $results "$Name-soak-frames.json"
$jsonl = Join-Path $results "$Name-soak.jsonl"
$summaryPath = Join-Path $results "$Name-soak.json"
Remove-Item $frames, $jsonl -ErrorAction Ignore

$seconds = $Hours * 3600
$fg0 = [SpikeSampler]::GetForegroundWindow()
$runArgs = @('--background', '--exit-after', "$([math]::Ceiling($seconds + 600))", '--frames', $frames) + $AppArgs
$started = Get-Date
$null = Get-CyclesPerSecond
Start-ForegroundWatch
$proc = Start-Spike (Join-Path $spike $Exe) $runArgs $spike
Write-Host "soak $Name pid $($proc.Id): spike.exe $($runArgs -join ' ') for $Hours h, every $Every s; $jsonl"
$rows = [Collections.Generic.List[object]]::new()
$ended = 'completed'
$spikeFront = 0
$allChanges = 0
try {
    Start-Sleep -Seconds ([math]::Min($Every, 10))
    $windows = Assert-OffScreen $proc.Id 'soak:'
    $prev = Get-TreeSnapshot $proc $frames
    $t0 = [Diagnostics.Stopwatch]::StartNew()
    for ($e = $Every; $e -le $seconds + 0.001; $e += $Every) {
        Start-Sleep -Milliseconds ([math]::Max(0, $e * 1000 - $t0.ElapsedMilliseconds))
        if ($proc.HasExited) { $ended = "the spike exited (code $($proc.ExitCode))"; break }
        $now = Get-TreeSnapshot $proc $frames
        $r = Compare-Snapshot $prev $now $proc.Id $TopThreads $fg0
        # Every foreground change since the last sample (SetWinEventHook), exe names only.
        $changes = @([SpikeForeground]::Drain() | ForEach-Object { $f = $_ -split '\|'; [pscustomobject]@{ at = $f[0]; pid = [int]$f[2]; exe = $f[3] } })
        $allChanges += $changes.Count
        $spikeFront += @($changes | Where-Object { $_.pid -eq $proc.Id }).Count
        $line = [ordered]@{
            t = [math]::Round($e); at = (Get-Date -Format o)
            spikeCpuPct = $r.spikeCpuPct; spikeCyclePct = $r.spikeCyclePct; treeCpuPct = $r.treeCpuPct; treeCyclePct = $r.treeCyclePct
            spikePrivateMB = $r.spikePrivateMB; spikeWorkingSetMB = $r.spikeWorkingSetMB
            spikeHandles = $r.spikeHandles; spikeThreads = $r.spikeThreads
            mainRendersPerSec = $r.mainRendersPerSec; editorRendersPerSec = $r.editorRendersPerSec
            topThreads = $r.topThreads; processes = $r.processes
            foreground = $r.foreground; foregroundUnchanged = $r.foregroundUnchanged; foregroundIsSpike = $r.foregroundIsSpike
            foregroundChanges = $changes
        }
        $rows.Add([pscustomobject]$line)
        ([pscustomobject]$line | ConvertTo-Json -Depth 5 -Compress) | Add-Content $jsonl
        Write-Host ("t={0,6:n0}s spike {1,5:n2}% cycles {6,5:n2}% tree {2,5:n2}% private {3:n1} MB handles {4} fg ok {5}" -f
            $e, $r.spikeCpuPct, $r.treeCpuPct, $r.spikePrivateMB, $r.spikeHandles, ($r.foregroundUnchanged -and -not $r.foregroundIsSpike), $r.spikeCyclePct)
        $prev = $now
    }
} catch {
    $ended = "stopped: $_"
} finally {
    if (-not $proc.HasExited) { $proc.Kill($true) }
    $proc.WaitForExit(10000) | Out-Null
    $last = Stop-ForegroundWatch
    $allChanges += $last.Count
    $spikeFront += @($last | Where-Object { $_.pid -eq $proc.Id }).Count

    $after = @($rows | Where-Object { $_.t -ge $WarmMinutes * 60 })
    $cpu = @($after | ForEach-Object spikeCpuPct | Where-Object { $null -ne $_ } | Sort-Object)
    $cyc = @($after | ForEach-Object spikeCyclePct | Where-Object { $null -ne $_ } | Sort-Object)
    $priv = @($after | ForEach-Object spikePrivateMB)
    $p95 = if ($cpu.Count) { $cpu[[math]::Min($cpu.Count - 1, [math]::Ceiling(0.95 * $cpu.Count) - 1)] } else { $null }
    $summary = [ordered]@{
        name = $Name; args = $runArgs -join ' '; started = $started.ToString('o'); ended = $ended
        samples = $rows.Count; samplesAfterWarmup = $after.Count; warmupMinutes = $WarmMinutes
        privateMBMin = ($priv | Measure-Object -Minimum).Minimum
        privateMBMax = ($priv | Measure-Object -Maximum).Maximum
        privateMBDelta = if ($priv.Count) { [math]::Round($priv[-1] - $priv[0], 1) } else { $null }
        spikeCpuPctMean = if ($cpu.Count) { [math]::Round(($cpu | Measure-Object -Average).Average, 3) } else { $null }
        spikeCpuPctP95 = $p95
        spikeCyclePctMean = if ($cyc.Count) { [math]::Round(($cyc | Measure-Object -Average).Average, 3) } else { $null }
        spikeCyclePctP95 = if ($cyc.Count) { $cyc[[math]::Min($cyc.Count - 1, [math]::Ceiling(0.95 * $cyc.Count) - 1)] } else { $null }
        cyclesPerSecondM = [math]::Round((Get-CyclesPerSecond) / 1e6, 1)
        treeCpuPctMean = if ($after.Count) { [math]::Round(($after | Measure-Object treeCpuPct -Average).Average, 3) } else { $null }
        handlesFirstLast = if ($after.Count) { "$($after[0].spikeHandles) -> $($after[-1].spikeHandles)" } else { $null }
        mainRendersPerSecMax = ($after | Measure-Object mainRendersPerSec -Maximum).Maximum
        foregroundBefore = Get-WindowLabel $fg0 $proc.Id
        foregroundChanges = $allChanges
        foregroundSpikeChanges = $spikeFront
        foregroundChangedSamples = @($rows | Where-Object { -not $_.foregroundUnchanged }).Count
        foregroundSpikeSamples = @($rows | Where-Object foregroundIsSpike).Count
    }
    [pscustomobject]$summary | ConvertTo-Json | Set-Content $summaryPath
    Write-Host ("{0}: {1}; after {2} min warm-up ({3} samples): private {4}-{5} MB (delta {6}), spike CPU mean {7}% p95 {8}% (cycles mean {14}% p95 {15}%), tree mean {9}%, handles {10}; foreground changed in {11} samples, the spike in {12}; {13}" -f
        $Name, $ended, $WarmMinutes, $after.Count, $summary.privateMBMin, $summary.privateMBMax, $summary.privateMBDelta,
        $summary.spikeCpuPctMean, $summary.spikeCpuPctP95, $summary.treeCpuPctMean, $summary.handlesFirstLast,
        $summary.foregroundChangedSamples, $summary.foregroundSpikeSamples, $summaryPath, $summary.spikeCyclePctMean, $summary.spikeCyclePctP95)
}
if ($summary.foregroundSpikeSamples -gt 0 -or $spikeFront -gt 0) { throw 'the spike came to the foreground' }
