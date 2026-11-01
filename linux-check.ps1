# Builds the crate for Linux in Docker and runs its whole unit test suite there:
# the Unix code (keeper.rs, parts of procs.rs and pty.rs) that test-build.ps1
# -Test, with its Windows binaries, never compiles.
#
#   ./linux-check.ps1
#
# The checkout goes in read-only, so the check leaves nothing behind in it. Runs
# under pwsh on any host with Docker.
$ErrorActionPreference = 'Stop'

$root = $PSScriptRoot

$build = (git -C $root rev-parse --short HEAD).Trim()
if (git -C $root status --porcelain) { $build = "$build+" }

docker build -t luna-linuxcheck -f "$root/docker/linux-check.Dockerfile" "$root/docker"
if ($LASTEXITCODE) { throw "docker build failed (exit $LASTEXITCODE)" }

# Single-quoted: the container's shell owns every $ in here.
#
# - The tests need no frontend, and the build only wants a folder to embed.
# - Every checkout shares the target volume under the same /app path, and cargo
#   goes by file times: a checkout whose sources are older than the last build
#   would pass for up to date. So the crate itself is always rebuilt; its
#   dependencies stay cached.
$cmd = @'
set -e
mkdir -p /tmp/no-frontend
export TAURI_CONFIG='{"build":{"frontendDist":"/tmp/no-frontend"}}'
cd /app/src-tauri
cargo clean --locked -p luna
cargo test --locked --lib --bins
'@ -replace "`r`n", "`n"

# One build at a time on this machine (build-windows.ps1 says why). These volumes
# are this script's own, but the machine's CPU and Docker's disk are not.
$lock = [System.Threading.Mutex]::new($false, 'Global\luna-docker-build')
$held = $false
try {
    try {
        if (-not $lock.WaitOne(0)) {
            Write-Host 'another Luna build is running; waiting for it'
            [void]$lock.WaitOne()
        }
    } catch [System.Threading.AbandonedMutexException] {
        # A build that died holding it: the mutex is ours now.
    }
    $held = $true
    # --init: the keeper's tests leave orphans to PID 1, and only a real init
    # reaps them; otherwise they stay zombies and look alive to kill(pid, 0).
    # The tmpfs: tauri-build rewrites gen/schemas, for this platform's view of
    # them, on every build.
    docker run --rm --init `
      -e LUNA_BUILD=$build `
      -e LUNA_FLAVOR=test `
      -e CARGO_TARGET_DIR=/target `
      -v "${root}:/app:ro" `
      --tmpfs /app/src-tauri/gen `
      -v luna-linux-target:/target `
      -v luna-linux-cargo-registry:/usr/local/cargo/registry `
      -w /app `
      luna-linuxcheck `
      bash -c $cmd
    if ($LASTEXITCODE) { throw "linux check failed (exit $LASTEXITCODE)" }
} finally {
    if ($held) { $lock.ReleaseMutex() }
    $lock.Dispose()
}
