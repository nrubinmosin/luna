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
    Start-Process -FilePath $exe -WorkingDirectory $out
}
