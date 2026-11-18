#Requires -Version 7.4
# Waits until the 4-hour soaks have written their summaries
# (results/soak4h-*-soak.json), then measures one variant idle for -Minutes
# with measure-idle.ps1 (both CPU figures, no --frames writer), on a copy of
# its exe in .soak/ so a rebuild cannot touch it. Gives up at -Until.
#
#   pwsh ./after-soak.ps1 -Variant fd -Exe .soak/idle-fd.exe
param(
    [string]$Variant = 'fd',
    [string]$Exe = '.soak/idle-fd.exe',
    [double]$Minutes = 30,
    [datetime]$Until = (Get-Date).AddHours(6),
    [string[]]$AppArgs = @('--term', '--pty', '--editor', 'samples/sample.md')
)
$ErrorActionPreference = 'Stop'
$spike = $PSScriptRoot
$want = @('soak4h-fd', 'soak4h-base') | ForEach-Object { Join-Path $spike "results/$_-soak.json" }
Write-Host "$(Get-Date -Format o) waiting for $($want -join ', ')"
while (($want | Where-Object { -not (Test-Path $_) }) -and (Get-Date) -lt $Until) { Start-Sleep -Seconds 30 }
if ($want | Where-Object { -not (Test-Path $_) }) { Write-Host "$(Get-Date -Format o) gave up: the soaks wrote no summary"; exit 1 }
Write-Host "$(Get-Date -Format o) soaks done; measuring $Variant idle for $Minutes min"
& (Join-Path $spike 'measure-idle.ps1') -Name "idle30-$Variant" -Exe $Exe -NoFrames -Warm 60 -Seconds ($Minutes * 60) -Every 60 -AppArgs ($AppArgs -join ',')
Write-Host "$(Get-Date -Format o) done"
