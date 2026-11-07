#Requires -Version 7.4
# Builds the test flavor of Luna into test-build\ (ignored by git) and,
# with -Run, starts it. It runs beside the Luna in use without touching it:
# its own identifier (single-instance lock, webview storage), and its log,
# attachments, settings and downloaded CLIs all live next to its exe.
#
#   ./scripts/test-build.ps1                 build test-build\luna-test.exe
#   ./scripts/test-build.ps1 -Run            build, then start it
#   ./scripts/test-build.ps1 -Run -NoBuild   start the last build
#   ./scripts/test-build.ps1 -Test           also build the Rust unit tests in Docker and
#                                            run them here (windows-tests.ps1)
#   ./scripts/test-build.ps1 -Run -DevCaller -DevFolder C:\src\x -DevAccount test-build\accounts\anthropic\dev
#       also registers a scripted MCP caller (agents/registry.rs register_dev_caller);
#       the token is printed and the MCP url is in test-build\luna.log
#   ./scripts/test-build.ps1 -Run -Background
#       the window off screen, unfocused and off the taskbar (LUNA_TEST_WINDOW,
#       lib.rs background), out of the way of whoever is at the screen; the
#       tray icon brings it up. -DevCaller implies it unless -Foreground is given.
#
# -Run also keeps a "Luna Test" shortcut in the Start menu carrying the test
# identifier, as the installer does for Luna: Windows drops the toasts of an
# app id no shortcut names, so the test build would show no notifications.
#
# Same Docker toolchain as build-windows.ps1, with its own cargo target dir so
# a test build never overwrites the release exe.
param(
    [switch]$Run,
    [switch]$Test,
    [switch]$NoBuild,
    [switch]$DevCaller,
    [switch]$Background,
    [switch]$Foreground,
    [string]$DevFolder = '',
    [string]$DevAccount = ''
)
. "$PSScriptRoot/lib.ps1"

$root = $RepoRoot
$out = Join-Path $root 'test-build'
New-Item -ItemType Directory -Force $out | Out-Null

# test-build\ is the test build's data folder, and Luna refuses a helper folder
# inside its data folder or above it (the repo itself; agents/privilege.rs
# `reaches_luna`): said before a build rather than by the dev caller's first
# spawn.
if ($DevFolder) {
    $outFull = [IO.Path]::GetFullPath($out).TrimEnd('\') + '\'
    $folderFull = [IO.Path]::GetFullPath($DevFolder, (Get-Location).Path).TrimEnd('\') + '\'
    $ignoreCase = [StringComparison]::OrdinalIgnoreCase
    if ($folderFull.StartsWith($outFull, $ignoreCase) -or $outFull.StartsWith($folderFull, $ignoreCase)) {
        throw "-DevFolder must be outside $outFull (the test build's data folder) and not hold it: Luna refuses helper folders there, and $DevFolder is one"
    }
}

if (-not $NoBuild) {
    $build = Get-BuildStamp
    Write-Host "test build stamp: $build"

    Use-BuildLock {
        $image = Build-Image luna-winbuild windows-build.Dockerfile
        Invoke-WinBuild 'test build' $build $image @'
pnpm install --frozen-lockfile
pnpm tauri build --runner cargo-xwin --target x86_64-pc-windows-msvc --no-bundle --config src-tauri/tauri.test.conf.json -- --locked
'@ $TestBuildEnv
    }

    Copy-Item -Force (Join-Path $out 'target/x86_64-pc-windows-msvc/release/luna.exe') (Join-Path $out 'luna-test.exe')
    Write-Host "built $out\luna-test.exe"
}

if ($Test) {
    & "$PSScriptRoot/windows-tests.ps1" -Build:(-not $NoBuild) -Run
}

if ($Run) {
    $exe = Join-Path $out 'luna-test.exe'
    if (-not (Test-Path $exe)) { throw "no $exe yet - run without -NoBuild first" }
    # The test build's accounts are throwaway ones. A junction or symlink there
    # to a real account folder once had test helpers write their transcripts
    # into the user's own account; refused rather than followed.
    $accounts = Join-Path $out 'accounts'
    if (Test-Path $accounts) {
        $linked = Get-ChildItem $accounts -Recurse -Depth 2 -Force -Attributes ReparsePoint -ErrorAction SilentlyContinue
        if ($linked) {
            throw "test-build\accounts holds links, which may lead to real accounts: $($linked.FullName -join ', '). Use throwaway accounts there instead."
        }
    }
    if ($DevCaller) {
        if (-not $DevFolder -or -not $DevAccount) { throw '-DevCaller needs -DevFolder and -DevAccount' }
        # The scripted caller spends its account's quota and writes into it:
        # only a throwaway account under test-build\accounts will do.
        $accountsFull = [IO.Path]::GetFullPath($accounts).TrimEnd('\') + '\'
        $devFull = [IO.Path]::GetFullPath($DevAccount, (Get-Location).Path)
        if (-not $devFull.StartsWith($accountsFull, [StringComparison]::OrdinalIgnoreCase)) {
            throw "-DevAccount must be an account under $accountsFull, not $devFull"
        }
        $token = [guid]::NewGuid().ToString('N')
        $env:LUNA_DEV_TOKEN = $token
        $env:LUNA_DEV_FOLDER = $DevFolder
        $env:LUNA_DEV_ACCOUNT = $DevAccount
        Write-Host "dev caller token: $token  (MCP url: see test-build\luna.log)"
    }
    if ($Background -and $Foreground) { throw '-Background and -Foreground exclude each other' }
    # Set or cleared every run: the started exe inherits this shell's environment.
    if ($Background -or ($DevCaller -and -not $Foreground)) {
        $env:LUNA_TEST_WINDOW = 'background'
    } else {
        Remove-Item Env:LUNA_TEST_WINDOW -ErrorAction SilentlyContinue
    }
    # The shortcut's System.AppUserModel.ID is what ties toasts to it; neither
    # WScript.Shell nor any cmdlet sets that property, hence the COM below.
    Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using System.Runtime.InteropServices.ComTypes;
public static class AppIdShortcut {
    [ComImport, Guid("00021401-0000-0000-C000-000000000046")] class ShellLink {}
    [ComImport, InterfaceType(ComInterfaceType.InterfaceIsIUnknown), Guid("000214F9-0000-0000-C000-000000000046")]
    interface IShellLinkW {
        void GetPath(IntPtr a, int b, IntPtr c, uint d); void GetIDList(out IntPtr a); void SetIDList(IntPtr a);
        void GetDescription(IntPtr a, int b); void SetDescription([MarshalAs(UnmanagedType.LPWStr)] string a);
        void GetWorkingDirectory(IntPtr a, int b); void SetWorkingDirectory([MarshalAs(UnmanagedType.LPWStr)] string a);
        void GetArguments(IntPtr a, int b); void SetArguments([MarshalAs(UnmanagedType.LPWStr)] string a);
        void GetHotkey(out short a); void SetHotkey(short a); void GetShowCmd(out int a); void SetShowCmd(int a);
        void GetIconLocation(IntPtr a, int b, out int c); void SetIconLocation([MarshalAs(UnmanagedType.LPWStr)] string a, int b);
        void SetRelativePath([MarshalAs(UnmanagedType.LPWStr)] string a, uint b); void Resolve(IntPtr a, uint b);
        void SetPath([MarshalAs(UnmanagedType.LPWStr)] string a);
    }
    [ComImport, InterfaceType(ComInterfaceType.InterfaceIsIUnknown), Guid("886D8EEB-8CF2-4446-8D02-CDBA1DBDCF99")]
    interface IPropertyStore {
        void GetCount(out uint a); void GetAt(uint a, out Key b); void GetValue(ref Key a, out Variant b);
        void SetValue(ref Key a, ref Variant b); void Commit();
    }
    [StructLayout(LayoutKind.Sequential, Pack = 4)] struct Key { public Guid Fmtid; public uint Pid; }
    [StructLayout(LayoutKind.Explicit)] struct Variant { [FieldOffset(0)] public ushort Vt; [FieldOffset(8)] public IntPtr Str; }
    public static void Save(string lnk, string exe, string appId) {
        var link = (IShellLinkW)new ShellLink();
        link.SetPath(exe);
        link.SetWorkingDirectory(System.IO.Path.GetDirectoryName(exe));
        link.SetIconLocation(exe, 0);
        var key = new Key { Fmtid = new Guid("9F4C2855-9F79-4B39-A8D0-E1D42DE1D5F3"), Pid = 5 };
        var value = new Variant { Vt = 31, Str = Marshal.StringToCoTaskMemUni(appId) };
        try {
            var store = (IPropertyStore)link;
            store.SetValue(ref key, ref value);
            store.Commit();
            ((IPersistFile)link).Save(lnk, true);
        } finally {
            Marshal.FreeCoTaskMem(value.Str);
        }
    }
}
'@
    $appId = (Get-Content (Join-Path $root 'src-tauri/tauri.test.conf.json') -Raw | ConvertFrom-Json).identifier
    $lnk = Join-Path ([Environment]::GetFolderPath('Programs')) 'Luna Test.lnk'
    [AppIdShortcut]::Save($lnk, $exe, $appId)
    Start-Process -FilePath $exe -WorkingDirectory $out
}
