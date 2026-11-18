#Requires -Version 7.4
# A/B of the terminal burst: runs each variant's exe on `--term --feed <feed>
# --term-stats ...`, off screen and never activated, interleaved (a, b, c, then
# c, b, a, ...) so that what else the desktop does falls on all of them alike:
# -Rounds rounds of -Feed, then one round of each of -ExtraFeeds. During each
# run it samples the desktop's CPU and the top consumers other than the spike
# (names only). With -QuietPct, each run waits first until the desktop (every
# process but the spike soaks, this script and the spike) has stayed under
# -QuietPct % of all cores for -QuietSeconds, giving up after -QuietDeadline
# minutes for the whole script. A foreground watcher runs throughout; a run
# fails if the spike ever came to the foreground or its window was on a
# display. Writes per-run JSON under results/ab/<Name>/, a summary
# results/ab/<Name>.json, and prints a table. Kills only what it started.
#
#   pwsh ./ab-burst.ps1 -Name fix -Rounds 4 -Variants 'base=spike-base.exe','fd=spike.exe'
#   pwsh ./ab-burst.ps1 -Name quiet -Rounds 4 -ExtraFeeds 'burst:60x4' -QuietPct 15 -QuietSeconds 120 -QuietDeadline 180
param(
    [string]$Name = 'ab',
    [int]$Rounds = 4,
    [string[]]$Variants = @('base=spike-base.exe', 'fd=spike.exe'),
    [string]$Feed = 'burst:60',
    [string[]]$ExtraFeeds = @(),
    [int]$SampleEvery = 5,
    [double]$QuietPct = 0,
    [int]$QuietSeconds = 120,
    [double]$QuietDeadline = 180
)
$ErrorActionPreference = 'Stop'
$Variants = @($Variants | ForEach-Object { $_ -split ',' } | Where-Object { $_ })
$ExtraFeeds = @($ExtraFeeds | ForEach-Object { $_ -split ',' } | Where-Object { $_ })
$spike = $PSScriptRoot
. (Join-Path $spike 'sampler.ps1')
$dir = Join-Path $spike "results/ab/$Name"
New-Item -ItemType Directory -Force $dir | Out-Null
# (PowerShell names are case-blind: $Variants is the parameter, $runs the parsed list.)
$runs = @($Variants | ForEach-Object { $pair = $_ -split '=', 2; [pscustomobject]@{ name = $pair[0]; exe = (Join-Path $spike $pair[1]) } })
foreach ($v in $runs) { if (-not (Test-Path $v.exe)) { throw "no $($v.exe)" } }
$cores = [Environment]::ProcessorCount
$deadline = (Get-Date).AddMinutes($QuietDeadline)

# CPU seconds by process id, of every process but the ones named in $Skip
# and this script.
function Get-CpuById([string[]]$Skip = @()) {
    $m = @{}
    foreach ($p in Get-Process) {
        if ($p.Id -eq $PID -or $p.ProcessName -eq 'Idle' -or ($Skip | Where-Object { $p.ProcessName -like $_ })) { continue }
        try { $m[$p.Id] = @($p.ProcessName, $p.TotalProcessorTime.TotalSeconds) } catch {}
    }
    $m
}
# % of all cores the processes in $Now used since $Was, and the top 5 by name.
function Measure-Desktop($Was, $Now, [double]$Seconds) {
    $byName = @{}; $sum = 0
    foreach ($id in $Now.Keys) {
        $d = $Now[$id][1] - $(if ($Was.ContainsKey($id)) { $Was[$id][1] } else { $Now[$id][1] })
        if ($d -le 0) { continue }
        $sum += $d; $byName[$Now[$id][0]] = ($byName[$Now[$id][0]] ?? 0) + $d
    }
    [pscustomobject]@{
        pct = [math]::Round(100 * $sum / ($Seconds * $cores), 1)
        top = (($byName.GetEnumerator() | Sort-Object Value -Descending | Select-Object -First 5 | ForEach-Object { "$($_.Key) $([math]::Round(100 * $_.Value / $Seconds, 1))" }) -join ', ')
    }
}
$soakSkip = @('spike-soak-*')

# Waits until the desktop has been quiet for $QuietSeconds (sampled every
# $SampleEvery s); returns the quiet window's mean %, or $null past the deadline.
function Wait-Quiet {
    $window = [Collections.Generic.List[double]]::new()
    $need = [math]::Ceiling($QuietSeconds / $SampleEvery)
    $prev = Get-CpuById $soakSkip; $t = [Diagnostics.Stopwatch]::StartNew()
    while ((Get-Date) -lt $deadline) {
        Start-Sleep -Seconds $SampleEvery
        $now = Get-CpuById $soakSkip
        $m = Measure-Desktop $prev $now $t.Elapsed.TotalSeconds
        $prev = $now; $t.Restart()
        if ($m.pct -lt $QuietPct) { $window.Add($m.pct) } else { $window.Clear() }
        if ($window.Count -ge $need) { return [math]::Round(($window | Measure-Object -Average).Average, 1) }
    }
    $null
}

$plan = @(for ($r = 1; $r -le $Rounds; $r++) { [pscustomobject]@{ round = $r; feed = $Feed } })
$plan += @($ExtraFeeds | ForEach-Object -Begin { $k = $Rounds } -Process { $k++; [pscustomobject]@{ round = $k; feed = $_ } })
$rows = [Collections.Generic.List[object]]::new()
$gaveUp = $false
Start-ForegroundWatch
try {
    foreach ($step in $plan) {
        $order = @($runs); if (-not ($step.round % 2)) { [array]::Reverse($order) }
        foreach ($v in $order) {
            $quiet = $null
            if ($QuietPct -gt 0) {
                $quiet = Wait-Quiet
                if ($null -eq $quiet) { $gaveUp = $true; break }
            }
            $secs = [int](($step.feed -replace '^burst:(\d+).*$', '$1') -as [int]); if (-not $secs) { $secs = 60 }
            $stats = Join-Path $dir "$($v.name)-$($step.round).json"
            Remove-Item $stats -ErrorAction Ignore
            $was = Get-CpuById ($soakSkip + @([IO.Path]::GetFileNameWithoutExtension($v.exe)))
            $t0 = [Diagnostics.Stopwatch]::StartNew()
            $proc = Start-Spike $v.exe @('--background', '--term', '--feed', $step.feed, '--term-stats', $stats, '--exit-after', "$($secs + 10)") $spike
            $offscreen = $null
            try {
                Start-Sleep -Seconds 3
                $offscreen = (Assert-OffScreen $proc.Id "$($v.name) round $($step.round):") -join '; '
                while (-not $proc.HasExited -and $t0.Elapsed.TotalSeconds -lt $secs + 25) { Start-Sleep -Seconds 1 }
            } finally {
                if (-not $proc.HasExited) { $proc.Kill($true) }
                $proc.WaitForExit(10000) | Out-Null
            }
            $desk = Measure-Desktop $was (Get-CpuById ($soakSkip + @([IO.Path]::GetFileNameWithoutExtension($v.exe)))) $t0.Elapsed.TotalSeconds
            $front = Get-SpikeForeground ([SpikeForeground]::Drain() | ForEach-Object { $f = $_ -split '\|'; [pscustomobject]@{ pid = [int]$f[2]; exe = $f[3] } }) @($proc.Id)
            $j = if (Test-Path $stats) { Get-Content $stats -Raw | ConvertFrom-Json } else { $null }
            $row = [pscustomobject][ordered]@{
                round = $step.round; feed = $step.feed; variant = $v.name
                lateFrames = $j.late_frames; lateFramesDisplayVsync = $j.late_frames_display_vsync; displayHz = $j.display_hz
                frames = $j.frames; unshownAtEnd = $j.output_unshown_at_end
                latencyMax = $j.latency_ms.max; latencyP99 = $j.latency_ms.p99; latencyP50 = $j.latency_ms.p50
                frameMax = $j.frame_ms.max; frameP99 = $j.frame_ms.p99
                checksum = $j.checksum.match
                quietBeforePct = $quiet; desktopPct = $desk.pct; topOthers = $desk.top
                window = $offscreen; spikeInFront = $front.Count -gt 0
            }
            $rows.Add($row)
            Write-Host ("r{0} {1,-10} {2,-6} late {3,3} (display vsync {4,4}) latency max {5,6} p99 {6,6} frame max {7,6} p99 {8,6} unshown {9} checksum {10} quiet before {11}% during {12}% | {13}" -f
                $step.round, $step.feed, $v.name, $row.lateFrames, $row.lateFramesDisplayVsync, $row.latencyMax, $row.latencyP99, $row.frameMax, $row.frameP99, $row.unshownAtEnd, $row.checksum, $row.quietBeforePct, $row.desktopPct, $row.topOthers)
            if ($row.spikeInFront) { throw "$($v.name) round $($step.round): the spike came to the foreground" }
        }
        if ($gaveUp) { break }
    }
} finally {
    $null = Stop-ForegroundWatch
}

function Get-Median($xs) { $s = @($xs | Where-Object { $null -ne $_ } | Sort-Object); if (-not $s.Count) { return $null }; if ($s.Count % 2) { $s[($s.Count - 1) / 2] } else { ($s[$s.Count / 2 - 1] + $s[$s.Count / 2]) / 2 } }
$medians = foreach ($feedName in @($Feed) + $ExtraFeeds) {
    foreach ($v in $runs) {
        $rs = @($rows | Where-Object { $_.variant -eq $v.name -and $_.feed -eq $feedName })
        if (-not $rs.Count) { continue }
        [pscustomobject][ordered]@{
            feed = $feedName; variant = $v.name; runs = $rs.Count
            lateFrames = Get-Median ($rs.lateFrames); lateDisplayVsync = Get-Median ($rs.lateFramesDisplayVsync)
            latencyMax = Get-Median ($rs.latencyMax); latencyP99 = Get-Median ($rs.latencyP99)
            frameMax = Get-Median ($rs.frameMax); frameP99 = Get-Median ($rs.frameP99)
            checksumAll = -not ($rs | Where-Object { -not $_.checksum }); unshownNone = -not ($rs | Where-Object { $_.unshownAtEnd })
            quietBeforePct = Get-Median ($rs.quietBeforePct); desktopPct = Get-Median ($rs.desktopPct)
        }
    }
}
[pscustomobject]@{ gaveUp = $gaveUp; quietPct = $QuietPct; quietSeconds = $QuietSeconds; runs = $rows; medians = $medians } | ConvertTo-Json -Depth 5 | Set-Content (Join-Path $spike "results/ab/$Name.json")
if ($gaveUp) { Write-Host "gave up waiting for a quiet desktop at $(Get-Date -Format o) after $($rows.Count) runs" }
Write-Host "medians:"; $medians | Format-Table -AutoSize | Out-String -Width 250 | Write-Host
Write-Host "done $(Get-Date -Format o)"
