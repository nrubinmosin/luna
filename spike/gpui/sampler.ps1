# What the measuring scripts share (measure-idle, soak, ab-burst, term-check,
# shot): starting the spike, snapshots of its own process tree (by PID) and the
# difference of two, a check that its windows are on no display, and a
# foreground watcher. Dot-sourced. Reads processes, windows' positions and
# foreground changes only; sends nothing to any window. Another application's
# window is named by its exe only, never by its title.

if (-not ('SpikeSampler' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.Collections.Concurrent;
using System.Collections.Generic;
using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;
public static class SpikeSampler {
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] static extern IntPtr MonitorFromRect(ref RECT r, uint flags);
    delegate bool EnumProc(IntPtr h, IntPtr l);
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc cb, IntPtr l);
    [DllImport("kernel32.dll")] static extern IntPtr OpenThread(uint access, bool inherit, uint id);
    [DllImport("kernel32.dll")] static extern IntPtr OpenProcess(uint access, bool inherit, uint id);
    [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr h);
    [DllImport("kernel32.dll")] static extern int GetThreadDescription(IntPtr h, out IntPtr desc);
    [DllImport("kernel32.dll")] static extern IntPtr LocalFree(IntPtr p);
    [DllImport("kernel32.dll")] static extern bool QueryProcessCycleTime(IntPtr h, out ulong cycles);
    [DllImport("kernel32.dll")] static extern bool QueryThreadCycleTime(IntPtr h, out ulong cycles);
    [DllImport("kernel32.dll")] static extern IntPtr GetCurrentThread();
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
    public static uint Pid(IntPtr h) { uint p; GetWindowThreadProcessId(h, out p); return p; }
    public static string Title(IntPtr h) { var s = new StringBuilder(256); GetWindowTextW(h, s, 256); return s.ToString(); }
    public static string ExeName(uint pid) { try { return Process.GetProcessById((int)pid).ProcessName; } catch { return "?"; } }
    // The name a thread gave itself (Rust's std sets it), or "".
    public static string ThreadName(uint tid) {
        IntPtr h = OpenThread(0x0800, false, tid); // THREAD_QUERY_LIMITED_INFORMATION
        if (h == IntPtr.Zero) return "";
        try {
            IntPtr p;
            if (GetThreadDescription(h, out p) < 0 || p == IntPtr.Zero) return "";
            string s = Marshal.PtrToStringUni(p) ?? "";
            LocalFree(p);
            return s;
        } finally { CloseHandle(h); }
    }
    // CPU cycles a process or thread has run, charged exactly (not by timer tick); 0 if unreadable.
    public static ulong ProcessCycles(uint pid) {
        IntPtr h = OpenProcess(0x1000, false, pid); // PROCESS_QUERY_LIMITED_INFORMATION
        if (h == IntPtr.Zero) return 0;
        try { ulong c; return QueryProcessCycleTime(h, out c) ? c : 0; } finally { CloseHandle(h); }
    }
    public static ulong ThreadCycles(uint tid) {
        IntPtr h = OpenThread(0x0800, false, tid);
        if (h == IntPtr.Zero) return 0;
        try { ulong c; return QueryThreadCycleTime(h, out c) ? c : 0; } finally { CloseHandle(h); }
    }
    // Cycles per second of one busy core, measured by spinning this thread for `ms`.
    public static double CyclesPerSecond(int ms) {
        ulong c0, c1; var sw = Stopwatch.StartNew();
        QueryThreadCycleTime(GetCurrentThread(), out c0);
        while (sw.ElapsedMilliseconds < ms) { }
        QueryThreadCycleTime(GetCurrentThread(), out c1);
        return (c1 - c0) / sw.Elapsed.TotalSeconds;
    }
    // The process's visible top-level windows that touch a display, as "hwnd rect".
    public static string[] OnDisplay(uint pid) {
        var hits = new List<string>();
        EnumWindows((h, l) => {
            uint p; GetWindowThreadProcessId(h, out p);
            if (p == pid && IsWindowVisible(h)) {
                RECT r; GetWindowRect(h, out r);
                if (MonitorFromRect(ref r, 0) != IntPtr.Zero) // MONITOR_DEFAULTTONULL
                    hits.Add("0x" + h.ToString("x") + " " + r.L + "," + r.T + " " + (r.R - r.L) + "x" + (r.B - r.T));
            }
            return true;
        }, IntPtr.Zero);
        return hits.ToArray();
    }
    // The visible top-level windows of the process, as "hwnd rect".
    public static string[] Windows(uint pid) {
        var all = new List<string>();
        EnumWindows((h, l) => {
            uint p; GetWindowThreadProcessId(h, out p);
            if (p == pid && IsWindowVisible(h)) { RECT r; GetWindowRect(h, out r); all.Add("0x" + h.ToString("x") + " " + r.L + "," + r.T + " " + (r.R - r.L) + "x" + (r.B - r.T)); }
            return true;
        }, IntPtr.Zero);
        return all.ToArray();
    }
}
// Every foreground change, as the system reports it (SetWinEventHook,
// EVENT_SYSTEM_FOREGROUND, out of context: the callback runs on this
// watcher's own thread, from its message loop; nothing is injected anywhere).
public static class SpikeForeground {
    delegate void WinEventProc(IntPtr hook, uint ev, IntPtr hwnd, int obj, int child, uint thread, uint time);
    [DllImport("user32.dll")] static extern IntPtr SetWinEventHook(uint min, uint max, IntPtr mod, WinEventProc proc, uint pid, uint tid, uint flags);
    [DllImport("user32.dll")] static extern bool UnhookWinEvent(IntPtr h);
    [StructLayout(LayoutKind.Sequential)] struct MSG { public IntPtr h; public uint m; public IntPtr w, l; public uint t; public int x, y; }
    [DllImport("user32.dll")] static extern int GetMessageW(out MSG m, IntPtr h, uint a, uint b);
    [DllImport("user32.dll")] static extern bool PostThreadMessageW(uint tid, uint msg, IntPtr w, IntPtr l);
    [DllImport("kernel32.dll")] static extern uint GetCurrentThreadId();
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    static Thread thread; static uint tid; static WinEventProc proc;
    static ConcurrentQueue<string> events = new ConcurrentQueue<string>();
    public static void Start() {
        var ready = new ManualResetEventSlim();
        proc = (hook, ev, hwnd, obj, child, thr, time) => {
            uint pid; GetWindowThreadProcessId(hwnd, out pid);
            events.Enqueue(DateTime.Now.ToString("o") + "|0x" + hwnd.ToString("x") + "|" + pid + "|" + SpikeSampler.ExeName(pid));
        };
        thread = new Thread(() => {
            tid = GetCurrentThreadId();
            IntPtr hook = SetWinEventHook(3, 3, IntPtr.Zero, proc, 0, 0, 0); // EVENT_SYSTEM_FOREGROUND, WINEVENT_OUTOFCONTEXT
            ready.Set();
            MSG m; while (GetMessageW(out m, IntPtr.Zero, 0, 0) > 0) { }
            UnhookWinEvent(hook);
        });
        thread.IsBackground = true;
        thread.Start();
        ready.Wait();
    }
    // Every change since Start (or the last Drain): "time|hwnd|pid|exe".
    public static string[] Drain() { var list = new List<string>(); string s; while (events.TryDequeue(out s)) list.Add(s); return list.ToArray(); }
    public static void Stop() { if (thread != null) { PostThreadMessageW(tid, 0x0012, IntPtr.Zero, IntPtr.Zero); thread.Join(2000); thread = null; } } // WM_QUIT
}
'@
}

# A window, named by handle, pid and exe; by its title only when it is the spike's.
function Get-WindowLabel([IntPtr]$h, [int]$SpikeId = 0) {
    if ($h -eq [IntPtr]::Zero) { return '0 (none)' }
    $wpid = [SpikeSampler]::Pid($h)
    $label = "0x$($h.ToString('x')) pid $wpid $([SpikeSampler]::ExeName($wpid))"
    if ($SpikeId -and $wpid -eq $SpikeId) { $label += " '$([SpikeSampler]::Title($h))'" }
    $label
}

# CreateProcess, not ShellExecute, and no STARTF_USESHOWWINDOW, so the app's
# own SW_SHOWNOACTIVATE stands.
function Start-Spike([string]$ExePath, [string[]]$Arguments, [string]$WorkDir) {
    $psi = [Diagnostics.ProcessStartInfo]::new($ExePath)
    $psi.UseShellExecute = $false
    $psi.WorkingDirectory = $WorkDir
    foreach ($a in $Arguments) { $psi.ArgumentList.Add($a) }
    [Diagnostics.Process]::Start($psi)
}

# Starts watching foreground changes (on a thread of this process; see
# SpikeForeground). Stop-ForegroundWatch returns every change seen.
function Start-ForegroundWatch { [SpikeForeground]::Start() }
function Stop-ForegroundWatch {
    $events = [SpikeForeground]::Drain()
    [SpikeForeground]::Stop()
    @($events | ForEach-Object {
        $f = $_ -split '\|'
        [pscustomobject]@{ at = $f[0]; hwnd = $f[1]; pid = [int]$f[2]; exe = $f[3] }
    })
}
# The changes that made one of these pids the foreground: a run fails on any.
function Get-SpikeForeground($Changes, [int[]]$Pids) { @($Changes | Where-Object { $_.pid -in $Pids }) }

# Throws if any visible window of the spike touches a display (MonitorFromRect
# with MONITOR_DEFAULTTONULL): off screen is the whole point of --background.
function Assert-OffScreen([int]$SpikeId, [string]$Label = '') {
    $hits = [SpikeSampler]::OnDisplay([uint32]$SpikeId)
    if ($hits.Count) { throw "$Label the spike's window is on a display: $($hits -join '; ')" }
    [SpikeSampler]::Windows([uint32]$SpikeId)
}

# Cycles per second of one busy core (a 1 s spin of this thread), to turn cycle
# counts into % of one core; measured once per run of a script. Clock speeds
# vary between cores and over time (boost, power states), so it is an estimate
# of "one core busy", not an exact conversion.
$script:CyclesPerSecond = $null
function Get-CyclesPerSecond {
    if (-not $script:CyclesPerSecond) { $script:CyclesPerSecond = [SpikeSampler]::CyclesPerSecond(1000) }
    $script:CyclesPerSecond
}

# The process and its descendants (a terminal pane's pwsh and conhost among them).
function Get-TreeIds([int]$Root) {
    $all = Get-CimInstance Win32_Process -Property ProcessId, ParentProcessId
    $ids = [Collections.Generic.List[int]]::new(); $ids.Add($Root)
    for ($i = 0; $i -lt $ids.Count; $i++) {
        foreach ($p in $all | Where-Object { $_.ParentProcessId -eq $ids[$i] -and $_.ProcessId -ne $ids[$i] }) { $ids.Add([int]$p.ProcessId) }
    }
    $ids
}

$script:ModuleCache = @{}
# The module a start address falls in, from the process's module list (read once).
function Get-ModuleOf([Diagnostics.Process]$Proc, [long]$Address) {
    if (-not $script:ModuleCache.ContainsKey($Proc.Id)) {
        $mods = try { @($Proc.Modules | ForEach-Object { [pscustomobject]@{ name = $_.ModuleName; base = [long]$_.BaseAddress; end = [long]$_.BaseAddress + $_.ModuleMemorySize } }) } catch { @() }
        $script:ModuleCache[$Proc.Id] = $mods
    }
    $m = $script:ModuleCache[$Proc.Id] | Where-Object { $Address -ge $_.base -and $Address -lt $_.end } | Select-Object -First 1
    if ($m) { $m.name } elseif ($Address -eq 0) { '' } else { '?' }
}

# The frames file the app rewrites once a second (src/idle.rs, --frames).
# The app may be rewriting it as it is read: retry a torn read once.
function Read-Frames([string]$Path) {
    for ($i = 0; $i -lt 3; $i++) {
        try { return [IO.File]::ReadAllText($Path) | ConvertFrom-Json } catch { Start-Sleep -Milliseconds 20 }
    }
    $null
}

function Get-TreeSnapshot([Diagnostics.Process]$Proc, [string]$FramesPath = '') {
    $snap = [ordered]@{ at = [Diagnostics.Stopwatch]::GetTimestamp(); procs = @{}; threads = @{}; frames = $null }
    foreach ($id in Get-TreeIds $Proc.Id) {
        $p = Get-Process -Id $id -ErrorAction SilentlyContinue
        if (-not $p) { continue }
        $snap.procs[$id] = [pscustomobject]@{
            id = $id; name = $p.ProcessName; cpu = $p.TotalProcessorTime.TotalSeconds; cycles = [SpikeSampler]::ProcessCycles([uint32]$id)
            privateMB = [math]::Round($p.PrivateMemorySize64 / 1MB, 1); workingSetMB = [math]::Round($p.WorkingSet64 / 1MB, 1)
            handles = $p.HandleCount; threads = $p.Threads.Count
        }
        if ($id -eq $Proc.Id) {
            foreach ($t in $p.Threads) {
                try { $snap.threads[[int]$t.Id] = @{ cpu = $t.TotalProcessorTime.TotalSeconds; cycles = [SpikeSampler]::ThreadCycles([uint32]$t.Id); start = $t.StartAddress.ToInt64() } } catch {}
            }
        }
    }
    if ($FramesPath) { $snap.frames = Read-Frames $FramesPath }
    [pscustomobject]$snap
}

# One row: what happened between two snapshots of the same tree. Two CPU
# figures: cpuPct from TotalProcessorTime, which Windows charges by timer tick
# (15.6 ms), so short wakes between ticks go uncharged and an idle process reads
# low; cyclePct from QueryProcessCycleTime, charged exactly, as % of one busy
# core at the rate Get-CyclesPerSecond measured. A process counts from the
# first snapshot it is in: one that starts and exits between two snapshots (a
# short-lived child of the pane's shell, say) is not counted at all, and a new
# one only from its first snapshot on. The spike's own figures are its process
# alone, children never included.
# (PowerShell names are case-blind: no local here may be called $a or $b.)
function Compare-Snapshot($A, $B, [int]$SpikeId, [int]$TopThreads, [IntPtr]$Foreground0) {
    $secs = ($B.at - $A.at) / [Diagnostics.Stopwatch]::Frequency
    $cps = Get-CyclesPerSecond
    $procs = foreach ($id in $B.procs.Keys) {
        $now = $B.procs[$id]; $was = $A.procs[$id]
        $cpu = if ($was) { 100 * ($now.cpu - $was.cpu) / $secs } else { $null }
        $cyc = if ($was -and $now.cycles -and $was.cycles) { 100 * ($now.cycles - $was.cycles) / ($cps * $secs) } else { $null }
        [pscustomobject]@{ id = $id; name = $now.name; cpuPct = if ($null -ne $cpu) { [math]::Round($cpu, 3) } else { $null }; cyclePct = if ($null -ne $cyc) { [math]::Round($cyc, 3) } else { $null }; privateMB = $now.privateMB; workingSetMB = $now.workingSetMB; handles = $now.handles; threads = $now.threads }
    }
    $spike = $procs | Where-Object id -eq $SpikeId
    $proc = Get-Process -Id $SpikeId -ErrorAction SilentlyContinue
    $threads = foreach ($tid in $B.threads.Keys) {
        $prev = $A.threads[$tid]
        $d = $B.threads[$tid].cpu - ($(if ($prev) { $prev.cpu } else { 0 }))
        $dc = if ($prev) { [double]$B.threads[$tid].cycles - [double]$prev.cycles } else { 0 }
        if ($d -le 0 -and $dc -le 0) { continue }
        [pscustomobject]@{ id = $tid; cpuPct = [math]::Round(100 * $d / $secs, 3); cyclePct = [math]::Round(100 * $dc / ($cps * $secs), 3); name = [SpikeSampler]::ThreadName([uint32]$tid); module = if ($proc) { Get-ModuleOf $proc $B.threads[$tid].start } else { '?' } }
    }
    $fg = [SpikeSampler]::GetForegroundWindow()
    $rate = { param($k) if ($A.frames -and $B.frames -and $B.frames.t -gt $A.frames.t) { [math]::Round(($B.frames.$k - $A.frames.$k) / ($B.frames.t - $A.frames.t), 2) } else { $null } }
    [pscustomobject][ordered]@{
        seconds = [math]::Round($secs, 1)
        spikeCpuPct = if ($spike) { $spike.cpuPct } else { $null }
        spikeCyclePct = if ($spike) { $spike.cyclePct } else { $null }
        treeCpuPct = [math]::Round((($procs | Where-Object { $null -ne $_.cpuPct }) | Measure-Object cpuPct -Sum).Sum, 3)
        treeCyclePct = [math]::Round((($procs | Where-Object { $null -ne $_.cyclePct }) | Measure-Object cyclePct -Sum).Sum, 3)
        spikePrivateMB = $spike.privateMB; spikeWorkingSetMB = $spike.workingSetMB; spikeHandles = $spike.handles; spikeThreads = $spike.threads
        mainRenders = $B.frames.main_renders; editorRenders = $B.frames.editor_renders
        mainRendersPerSec = & $rate 'main_renders'; editorRendersPerSec = & $rate 'editor_renders'
        topThreads = @($threads | Sort-Object cyclePct -Descending | Select-Object -First $TopThreads)
        processes = @($procs)
        foreground = Get-WindowLabel $fg $SpikeId
        foregroundUnchanged = $fg -eq $Foreground0
        foregroundIsSpike = [SpikeSampler]::Pid($fg) -eq $SpikeId
    }
}
