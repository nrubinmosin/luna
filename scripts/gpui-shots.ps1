#Requires -Version 7.4
# Screenshots of the native window's demo scenes (crates/window/src/demo.rs),
# each one opened over a fake host, off screen and never activated, and its
# frame rendered to a PNG inside the app (Window::render_to_image). Nothing of
# a running Luna is touched, and no other window is: the harness photographs
# only its own.
#
#   ./scripts/gpui-shots.ps1                     every scene, both themes
#   ./scripts/gpui-shots.ps1 -Scenes core-down   those whose name holds it
#   ./scripts/gpui-shots.ps1 -NoBuild            with the harness built last time
#   ./scripts/gpui-shots.ps1 -Fit                the fit check, then the scenes at its sizes
#
# The harness is an exe of its own, luna-window-shots.exe (the window crate's
# `shots` feature), built in the Windows toolchain image like the test build
# and run here, on this machine's GPU and fonts. The release exe never carries
# it: render_to_image is behind gpui's test support. The shots go to -Out
# (relative to the repo root), named <scene>-<light|dark>.png.
#
# -Fit is the check that the window fits small screens: first the window
# crate's fit test (src/fit_tests.rs) in the Linux toolchain image, which
# fails when a control of any scene or popup is out of reach (off the window,
# cut off by the frame round it with no scrolling to it, or squeezed to
# nothing) at any of its sizes; then every scene shot at each of those sizes,
# into -Out/fit as <scene>-<theme>-<w>x<h>.png, to look at.
param(
    [string]$Out = 'test-build/shots/gpui',
    [string]$Scenes = '',
    [switch]$NoBuild,
    [switch]$Fit
)
. "$PSScriptRoot/lib.ps1"

$exe = Join-Path $RepoRoot 'test-build/luna-window-shots.exe'
if ($Fit) {
    $build = Get-BuildStamp
    Use-BuildLock {
        $image = Build-Image luna-linuxcheck linux-check.Dockerfile
        # Selectors are recorded only in a debug build: the test's own profile.
        Invoke-LinuxCargo 'fit check' $build $image 'cargo test --locked -p luna-window --lib fit_tests'
    }
}
if (-not $NoBuild) {
    $build = Get-BuildStamp
    Use-BuildLock {
        $image = Build-Image luna-winbuild windows-build.Dockerfile
        # The test build's target dir, already warm with gpui; the `shots`
        # profile is release without its slow link-time optimization.
        Invoke-WinBuild 'shots build' $build $image @'
cd src-tauri
cargo xwin build --locked --profile shots -p luna-window --features shots --bin luna-window-shots --target x86_64-pc-windows-msvc
'@ $TestBuildEnv
    }
    Copy-Item -Force (Join-Path $RepoRoot 'test-build/target/x86_64-pc-windows-msvc/shots/luna-window-shots.exe') $exe
}
if (-not (Test-Path $exe)) { throw "no $exe yet - run without -NoBuild first" }

$outDir = Join-Path $RepoRoot $Out
New-Item -ItemType Directory -Force $outDir | Out-Null
$filter = if ($Scenes) { @($Scenes) } else { @() }
if ($Fit) {
    $outDir = Join-Path $outDir 'fit'
    New-Item -ItemType Directory -Force $outDir | Out-Null
    # fit_tests.rs `SIZES`, in logical pixels.
    foreach ($size in '1280x800', '1000x700', '900x600', '1280x720', '1024x600', '1024x576', '853x480', '683x400') {
        Invoke-Checked "luna-window-shots at $size" { & $exe --size $size $outDir @filter | Out-Host }
    }
} else {
    Invoke-Checked 'luna-window-shots' { & $exe $outDir @filter | Out-Host }
}
Get-ChildItem $outDir -Filter *.png | Sort-Object Name | ForEach-Object { Write-Host "  $($_.Name)" }
