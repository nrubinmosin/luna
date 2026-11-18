#Requires -Version 7.4
# Starts spike.exe off screen and never activated (--background), finds its own
# window by its own PID, photographs it with PrintWindow(PW_RENDERFULLCONTENT)
# on that HWND into results/, checks the picture is not blank, and kills its own
# process. Takes no input and activates nothing; it logs the foreground window
# before and after, which must stay the same and never be the spike's.
#
#   pwsh ./shot.ps1 [-Name spike] [-Size 800x500] [-WarmSeconds 3]
#   pwsh ./shot.ps1 -Name editor -Title 'gpui spike editor' -IdleSeconds 10 `
#       -AppArgs '--editor','samples/sample.md'
#
# -Title picks the process's window by its title (default: its largest).
# -IdleSeconds then measures the process's CPU time over that long and lets the
# app quit itself through --exit-after, writing its --stats JSON to
# results/<Name>-stats.json (-Stats PATH, relative to this folder, instead).
# -Exe runs another build (spike-term.exe). -ExitAfter SECS sets --exit-after
# itself (the terminal's burst run:
#   pwsh ./shot.ps1 -Name term-burst -WarmSeconds 30 -IdleSeconds 36 -ExitAfter 70 `
#       -Stats results/burst.json -AppArgs '--feed','burst:60')
param(
    [string]$Name = 'spike',
    [string]$Size = '800x500',
    [double]$WarmSeconds = 3,
    [string]$Title = '',
    [double]$IdleSeconds = 0,
    [string[]]$AppArgs = @(),
    [double]$ExitAfter = 0,
    [string]$Stats = '',
    [string]$Exe = 'spike.exe'
)
# Through `pwsh -File` an array arrives as one comma-joined string.
$AppArgs = @($AppArgs | ForEach-Object { $_ -split ',' } | Where-Object { $_ })
$ErrorActionPreference = 'Stop'
$spike = $PSScriptRoot
$exe = Join-Path $spike $Exe
$results = Join-Path $spike 'results'
New-Item -ItemType Directory -Force $results | Out-Null
$png = Join-Path $results "$Name.png"
$log = Join-Path $results "$Name-shot.log"
$statsFile = if ($Stats) { Join-Path $spike $Stats } else { Join-Path $results "$Name-stats.json" }

Add-Type -ReferencedAssemblies System.Drawing.Common, System.Drawing.Primitives, System.Private.Windows.GdiPlus, System.Private.Windows.Core, System.Collections -TypeDefinition @'
using System;
using System.Drawing;
using System.Drawing.Imaging;
using System.Runtime.InteropServices;
using System.Text;
public static class SpikeWin {
    delegate bool EnumProc(IntPtr h, IntPtr l);
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc cb, IntPtr l);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] static extern bool PrintWindow(IntPtr h, IntPtr dc, uint flags);
    [DllImport("user32.dll")] static extern int GetWindowLongW(IntPtr h, int index);
    [DllImport("user32.dll")] static extern IntPtr MonitorFromRect(ref RECT r, uint flags);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
    // The largest visible top-level window of the process, so never another's.
    public static IntPtr Find(uint pid, string title) {
        IntPtr best = IntPtr.Zero; long area = 0;
        EnumWindows((h, l) => {
            uint p; GetWindowThreadProcessId(h, out p);
            if (p == pid && IsWindowVisible(h) && (title.Length == 0 || Title(h) == title)) {
                RECT r; GetWindowRect(h, out r);
                long a = (long)(r.R - r.L) * (r.B - r.T);
                if (a > area) { area = a; best = h; }
            }
            return true;
        }, IntPtr.Zero);
        return best;
    }
    public static uint Pid(IntPtr h) { uint p; GetWindowThreadProcessId(h, out p); return p; }
    public static string Title(IntPtr h) { var s = new StringBuilder(256); GetWindowTextW(h, s, 256); return s.ToString(); }
    public static string Rect(IntPtr h) { RECT r; GetWindowRect(h, out r); return r.L + "," + r.T + " " + (r.R - r.L) + "x" + (r.B - r.T); }
    public static string ExStyle(IntPtr h) { return "0x" + ((uint)GetWindowLongW(h, -20)).ToString("x8"); }
    // MONITOR_DEFAULTTONULL: zero when the window touches no display.
    public static bool OnAnyDisplay(IntPtr h) { RECT r; GetWindowRect(h, out r); return MonitorFromRect(ref r, 0) != IntPtr.Zero; }
    // Returns the number of distinct colors (capped) in the shot.
    public static int Shot(IntPtr h, string path) {
        RECT r; GetWindowRect(h, out r);
        using (var bmp = new Bitmap(r.R - r.L, r.B - r.T, PixelFormat.Format32bppArgb)) {
            using (var g = Graphics.FromImage(bmp)) {
                IntPtr dc = g.GetHdc();
                bool ok = PrintWindow(h, dc, 2); // PW_RENDERFULLCONTENT
                g.ReleaseHdc(dc);
                if (!ok) throw new Exception("PrintWindow failed");
            }
            bmp.Save(path, ImageFormat.Png);
            var colors = new System.Collections.Generic.HashSet<int>();
            for (int y = 0; y < bmp.Height; y += 2)
                for (int x = 0; x < bmp.Width; x += 2) {
                    colors.Add(bmp.GetPixel(x, y).ToArgb());
                    if (colors.Count >= 1000) return colors.Count;
                }
            return colors.Count;
        }
    }
}
'@

function Write-Log([string]$Line) {
    Write-Host $Line
    Add-Content -Path $log -Value $Line
}
Set-Content -Path $log -Value "shot.ps1 $(Get-Date -Format o)"

. (Join-Path $spike 'sampler.ps1')
# Another application's window by its exe only; the spike's with its title.
function Describe([IntPtr]$h) { Get-WindowLabel $h $(if ($proc) { $proc.Id } else { 0 }) }
$proc = $null

$fg0 = [SpikeWin]::GetForegroundWindow()
Write-Log "foreground before: $(Describe $fg0)"
$psi = [Diagnostics.ProcessStartInfo]::new($exe)
# CreateProcess, not ShellExecute, and no STARTF_USESHOWWINDOW, so the app's
# own SW_SHOWNOACTIVATE stands.
$psi.UseShellExecute = $false
$psi.WorkingDirectory = $spike
$exitAfter = if ($ExitAfter -gt 0) { $ExitAfter } elseif ($IdleSeconds -gt 0) { [math]::Ceiling($WarmSeconds + $IdleSeconds + 4) } else { 60 }
$runArgs = @('--background', '--size', $Size, '--exit-after', "$exitAfter") + $AppArgs
if ($IdleSeconds -gt 0) { Remove-Item $statsFile -ErrorAction Ignore; $runArgs += @('--stats', $statsFile) }
Write-Log "spike.exe $($runArgs -join ' ')"
foreach ($a in $runArgs) { $psi.ArgumentList.Add($a) }
Start-ForegroundWatch
$started = [Diagnostics.Stopwatch]::StartNew()
$proc = [Diagnostics.Process]::Start($psi)
$failed = $null
try {
    $hwnd = [IntPtr]::Zero
    for ($i = 0; $i -lt 100 -and $hwnd -eq [IntPtr]::Zero -and -not $proc.HasExited; $i++) {
        Start-Sleep -Milliseconds 100; $hwnd = [SpikeWin]::Find([uint32]$proc.Id, $Title)
    }
    if ($hwnd -eq [IntPtr]::Zero) { throw "no window from pid $($proc.Id) (exited: $($proc.HasExited))" }
    Write-Log "window after $($started.ElapsedMilliseconds) ms: $(Describe $hwnd) at $([SpikeWin]::Rect($hwnd)), exstyle $([SpikeWin]::ExStyle($hwnd)), on a display: $([SpikeWin]::OnAnyDisplay($hwnd))"
    # Every window of the spike off every display, or the run fails.
    Write-Log "the spike's windows: $((Assert-OffScreen $proc.Id) -join '; ')"
    Start-Sleep -Seconds $WarmSeconds
    $colors = [SpikeWin]::Shot($hwnd, $png)
    Write-Log "shot: $png, distinct colors (sampled, capped at 1000): $colors"
    if ($colors -lt 3) { $failed = "the shot is blank ($colors colors)" }
    $fg1 = [SpikeWin]::GetForegroundWindow()
    Write-Log "foreground after: $(Describe $fg1)"
    $same = $fg0 -eq $fg1
    $isSpike = ($fg1 -eq $hwnd) -or ([SpikeWin]::Pid($fg1) -eq $proc.Id)
    Write-Log "foreground unchanged: $same; foreground is the spike: $isSpike"
    if ($isSpike) { $failed = 'the spike came to the foreground' }
    if ($IdleSeconds -gt 0) {
        $cpu0 = $proc.TotalProcessorTime; $t0 = [Diagnostics.Stopwatch]::StartNew()
        Start-Sleep -Seconds $IdleSeconds
        $proc.Refresh()
        $pct = 100 * ($proc.TotalProcessorTime - $cpu0).TotalSeconds / $t0.Elapsed.TotalSeconds
        Write-Log ("idle CPU over {0:n1} s: {1:n2}% of one core" -f $t0.Elapsed.TotalSeconds, $pct)
        if (-not $proc.WaitForExit(15000)) { Write-Log 'did not quit by itself' }
        if (Test-Path $statsFile) { Write-Log "stats: $(Get-Content $statsFile -Raw)".Trim() } else { Write-Log 'no stats written' }
        $fg2 = [SpikeWin]::GetForegroundWindow()
        Write-Log "foreground at the end: $(Describe $fg2); unchanged: $($fg2 -eq $fg0)"
        if ([SpikeWin]::Pid($fg2) -eq $proc.Id) { $failed = 'the spike came to the foreground' }
    }
} finally {
    if (-not $proc.HasExited) { $proc.Kill($true) }
    $proc.WaitForExit(10000) | Out-Null
    Write-Log "spike pid $($proc.Id) exited: $($proc.HasExited)"
    # Every foreground change while it ran (SetWinEventHook): the operator's are
    # logged by exe; the spike's own fails the run.
    $changes = Stop-ForegroundWatch
    $front = Get-SpikeForeground $changes @($proc.Id)
    Write-Log "foreground changes while it ran: $($changes.Count) ($(($changes | ForEach-Object exe | Sort-Object -Unique) -join ', ')); to the spike: $($front.Count)"
    if ($front.Count) { $failed = 'the spike came to the foreground' }
}
if ($failed) { throw $failed }
