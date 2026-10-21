# Builds the test flavour of Luna into test-build\ (ignored by git) and,
# with -Run, starts it. It runs beside the Luna in use without touching it:
# its own identifier (single-instance lock, webview storage), and its log,
# attachments, settings and downloaded CLIs all live next to its exe.
#
#   ./test-build.ps1                 build test-build\luna-test.exe
#   ./test-build.ps1 -Run            build, then start it
#   ./test-build.ps1 -Run -NoBuild   start the last build
#   ./test-build.ps1 -Test           also build the Rust unit tests in Docker and
#                                    run them here (they are Windows binaries)
#   ./test-build.ps1 -Run -DevCaller -DevFolder C:\src\x -DevAccount E:\...\anthropic\team
#       also registers a scripted MCP caller (agents.rs register_dev_caller);
#       the token is printed and the MCP url is in test-build\luna.log
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
    [string]$DevFolder = '',
    [string]$DevAccount = ''
)
$ErrorActionPreference = 'Stop'

$root = $PSScriptRoot
$out = Join-Path $root 'test-build'
New-Item -ItemType Directory -Force $out | Out-Null

if (-not $NoBuild) {
    $build = (git -C $root rev-parse --short HEAD).Trim()
    if (git -C $root status --porcelain) { $build = "$build+" }
    Write-Host "test build stamp: $build"

    docker build -t luna-winbuild -f "$root/docker/windows-build.Dockerfile" "$root/docker"
    if ($LASTEXITCODE) { throw "docker build failed (exit $LASTEXITCODE)" }

    $cmd = @"
set -e
pnpm install --frozen-lockfile
pnpm tauri build --runner cargo-xwin --target x86_64-pc-windows-msvc --no-bundle --config src-tauri/tauri.test.conf.json
$(if ($Test) { 'cd src-tauri && cargo xwin test --release --no-run --target x86_64-pc-windows-msvc' })
"@ -replace "`r`n", "`n"

    docker run --rm `
      -e COREPACK_ENABLE_DOWNLOAD_PROMPT=0 `
      -e LUNA_BUILD=$build `
      -e LUNA_FLAVOR=test `
      -e VITE_LUNA_FLAVOR=test `
      -e CARGO_TARGET_DIR=/app/test-build/target `
      -v "${root}:/app" `
      -v luna-node-modules:/app/node_modules `
      -v luna-xwin-cache:/xwin-cache `
      -v luna-cargo-registry:/usr/local/cargo/registry `
      -v luna-tauri-cache:/root/.cache/tauri `
      -w /app `
      luna-winbuild `
      bash -c $cmd
    if ($LASTEXITCODE) { throw "test build failed (exit $LASTEXITCODE)" }

    Copy-Item -Force (Join-Path $out 'target/x86_64-pc-windows-msvc/release/luna.exe') (Join-Path $out 'luna-test.exe')
    Write-Host "built $out\luna-test.exe"
}

if ($Test) {
    $bin = Get-ChildItem (Join-Path $out 'target/x86_64-pc-windows-msvc/release/deps') -Filter 'luna_lib-*.exe' |
        Sort-Object LastWriteTime -Descending | Select-Object -First 1
    if (-not $bin) { throw 'no test binary - build with -Test first' }
    & $bin.FullName
    if ($LASTEXITCODE) { throw "unit tests failed (exit $LASTEXITCODE)" }
}

if ($Run) {
    $exe = Join-Path $out 'luna-test.exe'
    if (-not (Test-Path $exe)) { throw "no $exe yet - run without -NoBuild first" }
    if ($DevCaller) {
        if (-not $DevFolder -or -not $DevAccount) { throw '-DevCaller needs -DevFolder and -DevAccount' }
        $token = [guid]::NewGuid().ToString('N')
        $env:LUNA_DEV_TOKEN = $token
        $env:LUNA_DEV_FOLDER = $DevFolder
        $env:LUNA_DEV_ACCOUNT = $DevAccount
        Write-Host "dev caller token: $token  (MCP url: see test-build\luna.log)"
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
