#Requires -Version 7.4
# The terminal pane under the current spike.exe, off screen and never
# activated, in three runs:
#   burst   --feed burst:60 --stats, compared with results/burst.json (the
#           terminal's reference run): frames, frame ms max/p99, latency, checksum.
#   resize  --feed burst:20 --stats while this script resizes the spike's own
#           window (SetWindowPos with SWP_NOACTIVATE | SWP_NOZORDER | SWP_NOMOVE)
#           three times: the grid follows (stats.resizes, stats.grid) and the
#           checksum against a headless Term given the same resizes still matches.
#   pty     --pty, the window resized before --type has pwsh write its own
#           console size to a file: the PTY follows the pane's grid.
# Each run's window is photographed (PrintWindow on its own HWND) and the
# foreground window logged before and after; the spike in front fails the run.
# Writes results/<Name>-*.json|png|txt and results/<Name>.log.
#
#   pwsh ./term-check.ps1 [-Name termcheck] [-Exe spike.exe]
param(
    [string]$Name = 'termcheck',
    [string]$Exe = 'spike.exe',
    # Which runs, of burst, resize and pty.
    [string[]]$Only = @('burst', 'resize', 'pty')
)
$Only = @($Only | ForEach-Object { $_ -split ',' } | Where-Object { $_ })
$ErrorActionPreference = 'Stop'
$spike = $PSScriptRoot
. (Join-Path $spike 'sampler.ps1')
$results = Join-Path $spike 'results'
$log = Join-Path $results "$Name.log"
Set-Content $log "term-check.ps1 $Exe $(Get-Date -Format o)"
function Write-Log([string]$Line) { Write-Host $Line; Add-Content $log $Line }

Add-Type -ReferencedAssemblies System.Drawing.Common, System.Drawing.Primitives, System.Private.Windows.GdiPlus, System.Private.Windows.Core -TypeDefinition @'
using System;
using System.Drawing;
using System.Drawing.Imaging;
using System.Runtime.InteropServices;
using System.Text;
public static class TermWin {
    delegate bool EnumProc(IntPtr h, IntPtr l);
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc cb, IntPtr l);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] static extern bool GetClientRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] static extern bool PrintWindow(IntPtr h, IntPtr dc, uint flags);
    [DllImport("user32.dll")] static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int cx, int cy, uint flags);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
    static string Title(IntPtr h) { var s = new StringBuilder(256); GetWindowTextW(h, s, 256); return s.ToString(); }
    // The process's largest visible window that is not its editor window.
    public static IntPtr Find(uint pid) {
        IntPtr best = IntPtr.Zero; long area = 0;
        EnumWindows((h, l) => {
            uint p; GetWindowThreadProcessId(h, out p);
            if (p == pid && IsWindowVisible(h) && Title(h) != "gpui spike editor") {
                RECT r; GetWindowRect(h, out r);
                long a = (long)(r.R - r.L) * (r.B - r.T);
                if (a > area) { area = a; best = h; }
            }
            return true;
        }, IntPtr.Zero);
        return best;
    }
    public static string Client(IntPtr h) { RECT r; GetClientRect(h, out r); return (r.R - r.L) + "x" + (r.B - r.T); }
    // Outer size only: no move, no z-order change, no activation.
    public static bool Resize(IntPtr h, int w, int hgt) { return SetWindowPos(h, IntPtr.Zero, 0, 0, w, hgt, 0x0002 | 0x0004 | 0x0010); }
    public static void Shot(IntPtr h, string path) {
        RECT r; GetWindowRect(h, out r);
        using (var bmp = new Bitmap(r.R - r.L, r.B - r.T, PixelFormat.Format32bppArgb)) {
            using (var g = Graphics.FromImage(bmp)) { IntPtr dc = g.GetHdc(); PrintWindow(h, dc, 2); g.ReleaseHdc(dc); }
            bmp.Save(path, ImageFormat.Png);
        }
    }
}
'@

# Runs the spike with $RunArgs, hands its window to $During, waits for it to
# quit (its --exit-after), and returns whether the foreground was ever the spike
# (every change seen by the foreground watcher). Throws if its window is on a display.
function Invoke-Run([string]$Label, [string[]]$RunArgs, [scriptblock]$During, [int]$WaitSeconds) {
    $fg0 = [SpikeSampler]::GetForegroundWindow()
    Write-Log "[$Label] spike $($RunArgs -join ' '); foreground before: $(Get-WindowLabel $fg0)"
    Start-ForegroundWatch
    $proc = Start-Spike (Join-Path $spike $Exe) $RunArgs $spike
    $spikeInFront = $false
    try {
        $hwnd = [IntPtr]::Zero
        for ($i = 0; $i -lt 100 -and $hwnd -eq [IntPtr]::Zero -and -not $proc.HasExited; $i++) { Start-Sleep -Milliseconds 100; $hwnd = [TermWin]::Find([uint32]$proc.Id) }
        if ($hwnd -eq [IntPtr]::Zero) { throw "[$Label] no window" }
        Write-Log "[$Label] windows: $((Assert-OffScreen $proc.Id "[$Label]") -join '; ')"
        & $During $hwnd
        Assert-OffScreen $proc.Id "[$Label] after resizing:" | Out-Null
        $deadline = (Get-Date).AddSeconds($WaitSeconds)
        while (-not $proc.HasExited -and (Get-Date) -lt $deadline) {
            if ([SpikeSampler]::Pid([SpikeSampler]::GetForegroundWindow()) -eq $proc.Id) { $spikeInFront = $true }
            Start-Sleep -Milliseconds 500
        }
        if (-not $proc.HasExited) { Write-Log "[$Label] did not quit by itself; killed" }
    } finally {
        if (-not $proc.HasExited) { $proc.Kill($true) }
        $proc.WaitForExit(10000) | Out-Null
        $changes = Stop-ForegroundWatch
    }
    if ((Get-SpikeForeground $changes @($proc.Id)).Count) { $spikeInFront = $true }
    $fg1 = [SpikeSampler]::GetForegroundWindow()
    Write-Log "[$Label] foreground after: $(Get-WindowLabel $fg1); changes while it ran: $($changes.Count); ever the spike: $spikeInFront"
    $spikeInFront
}

function Format-Burst($j) {
    "frames $($j.frames), frame ms max $($j.frame_ms.max) p99 $($j.frame_ms.p99), latency ms max $($j.latency_ms.max) p99 $($j.latency_ms.p99), interval p99 $($j.interval_ms.p99), resizes $($j.resizes), grid $($j.grid.cols)x$($j.grid.rows), bytes equal $($j.bytes.equal), checksum match $($j.checksum.match)"
}

$failed = @()

# burst
if ('burst' -in $Only) {
$burstJson = Join-Path $results "$Name-burst.json"
Remove-Item $burstJson -ErrorAction Ignore
$bad = Invoke-Run 'burst' @('--background', '--feed', 'burst:60', '--stats', $burstJson, '--exit-after', '68') {
    param($h) Start-Sleep -Seconds 30; [TermWin]::Shot($h, (Join-Path $results "$Name-burst.png"))
} 60
if ($bad) { $failed += 'burst: foreground' }
$ref = Get-Content (Join-Path $results 'burst.json') -Raw | ConvertFrom-Json
$got = Get-Content $burstJson -Raw | ConvertFrom-Json
Write-Log "[burst] reference: $(Format-Burst $ref)"
Write-Log "[burst] this run:  $(Format-Burst $got)"
if (-not $got.checksum.match) { $failed += 'burst: checksum' }
}

# resize
if ('resize' -in $Only) {
$resizeJson = Join-Path $results "$Name-resize.json"
Remove-Item $resizeJson -ErrorAction Ignore
$bad = Invoke-Run 'resize' @('--background', '--feed', 'burst:20', '--stats', $resizeJson, '--exit-after', '28') {
    param($h)
    foreach ($s in @(@(1000, 640), @(640, 420), @(1200, 760))) {
        Start-Sleep -Seconds 3
        $ok = [TermWin]::Resize($h, $s[0], $s[1])
        Start-Sleep -Milliseconds 300
        Write-Log "[resize] SetWindowPos $($s[0])x$($s[1]): $ok, client now $([TermWin]::Client($h))"
    }
    Start-Sleep -Seconds 2
    [TermWin]::Shot($h, (Join-Path $results "$Name-resize.png"))
} 30
if ($bad) { $failed += 'resize: foreground' }
$got = Get-Content $resizeJson -Raw | ConvertFrom-Json
Write-Log "[resize] $(Format-Burst $got)"
if (-not $got.checksum.match) { $failed += 'resize: checksum' }
if ($got.resizes -lt 3) { $failed += "resize: $($got.resizes) resizes seen" }
}

# pty
if ('pty' -in $Only) {
$sizeFile = Join-Path $results "$Name-pty-size.txt"
Remove-Item $sizeFile -ErrorAction Ignore
# Relative, with forward slashes: --type turns every backslash-r into Enter,
# the one in "\results" too. The pane's shell starts in this folder.
$type = "'{0}x{1}' -f [Console]::WindowWidth, [Console]::WindowHeight | Set-Content -LiteralPath 'results/$Name-pty-size.txt'\r"
$bad = Invoke-Run 'pty' @('--background', '--pty', '--type', $type, '--exit-after', '12') {
    param($h)
    Start-Sleep -Milliseconds 1500
    $ok = [TermWin]::Resize($h, 1000, 640)
    $script:ptyClient = [TermWin]::Client($h)
    Write-Log "[pty] SetWindowPos 1000x640 before the typed command: $ok, client now $($script:ptyClient)"
    Start-Sleep -Seconds 6
    [TermWin]::Shot($h, (Join-Path $results "$Name-pty.png"))
} 15
if ($bad) { $failed += 'pty: foreground' }
$size = if (Test-Path $sizeFile) { (Get-Content $sizeFile -Raw).Trim() } else { $null }
Write-Log "[pty] pwsh in the pane reports its console as: $size"
if (-not $size) { $failed += 'pty: no size written' }
# The grid that client area holds, from the cell size a feed run measured
# (same font): what the PTY must have been told.
$cellRef = @($resizeJson, (Join-Path $results 'burst.json')) | Where-Object { $_ -and (Test-Path $_) } | Select-Object -First 1
if ($size -and $cellRef -and $script:ptyClient) {
    $g = (Get-Content $cellRef -Raw | ConvertFrom-Json).grid
    $cw, $chh = $script:ptyClient -split 'x' | ForEach-Object { [int]$_ }
    $expected = "{0}x{1}" -f [math]::Floor($cw / $g.cell_w), [math]::Floor($chh / $g.line_h)
    Write-Log "[pty] grid for a $($script:ptyClient) client at $($g.cell_w)x$($g.line_h) px cells: $expected; reported: $size"
    if ($size -ne $expected) { $failed += "pty: reported $size, grid $expected" }
}
}

if ($failed) { Write-Log "FAILED: $($failed -join '; ')"; throw ($failed -join '; ') }
Write-Log 'all runs passed'
