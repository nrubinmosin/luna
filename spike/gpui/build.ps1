#Requires -Version 7.4
# Builds spike.exe in Docker (no local Rust), cross-compiling from Linux with
# cargo-xwin, and copies it next to this script. -Tests builds the test
# executables instead (cargo xwin test --no-run) and copies them to tests/.
#
#   pwsh ./build.ps1
#   pwsh ./build.ps1 -Tests
#   pwsh ./build.ps1 -Features shot    -> spike-shot.exe (in-app --shot)
#   pwsh ./build.ps1 -Base             -> spike-base.exe (and tests-base/ with -Tests)
#
# -Base builds the branch base-vsync, the tree before the frames-on-demand patch
# to gpui-pre-windows (README.md "Idle cost"): the unpatched vsync loop, for
# comparison. It builds from its worktree .wt/base, made here if missing, with
# a cargo target dir of its own in the volume (/target/base): the two trees
# share paths inside the container, and cargo, going by file times, could
# otherwise take one tree's build for the other's.
#
# The image extends Luna's own Windows build image (docker/windows-build.Dockerfile
# in the repo), built through the repo's scripts/lib.ps1. The cargo target dir
# and the Windows SDK cache live in named volumes of their own
# (gpui-spike-target, gpui-spike-xwin-cache), so this never contends with a Luna
# build or the gpui-proto one; the crates.io registry volume is Luna's, which
# cargo locks itself. Remove the two volumes with
#   docker volume rm gpui-spike-target gpui-spike-xwin-cache
param([switch]$Tests, [string]$Features = '', [switch]$Base)
$ErrorActionPreference = 'Stop'
$spike = $PSScriptRoot
. (Join-Path $spike '../../scripts/lib.ps1')

$baseImage = Build-Image luna-winbuild windows-build.Dockerfile
Invoke-Checked 'docker build of gpui-spike' {
    docker build -t gpui-spike --build-arg "BASE=$baseImage" -f (Join-Path $spike 'docker/Dockerfile') (Join-Path $spike 'docker') | Out-Host
}

$source = $spike
$targetDir = '/target'
$variant = ''
if ($Base) {
    $source = Join-Path $spike '.wt/base'
    if (-not (Test-Path $source)) {
        Invoke-Checked 'git worktree add' { git -C $spike worktree add (Join-Path '.wt' 'base') base-vsync | Out-Host }
    }
    $targetDir = '/target/base'
    $variant = '-base'
}
$mode = if ($Tests) { 'tests' } else { 'exe' }
Invoke-Checked "cargo xwin ($mode)" {
    docker run --rm `
      -e CARGO_TARGET_DIR=$targetDir `
      -e SPIKE_VARIANT=$variant `
      -e CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS="-C link-arg=/IGNORE:4099" `
      -v "${source}:/proj" `
      -v "${spike}:/out" `
      -v gpui-spike-target:/target `
      -v gpui-spike-xwin-cache:/xwin-cache `
      -v luna-cargo-registry:/usr/local/cargo/registry `
      -w /proj `
      gpui-spike `
      bash /out/docker/build.sh $mode $Features
}
