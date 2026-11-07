# Builds the crate for Linux in Docker and runs its whole unit test suite there:
# the Unix code (keeper.rs, parts of procs.rs and pty.rs) that test-build.ps1
# -Test, with its Windows binaries, never compiles.
#
#   ./scripts/linux-check.ps1
#
# The checkout goes in read-only, so the check leaves nothing behind in it. Runs
# under pwsh on any host with Docker.
. "$PSScriptRoot/lib.ps1"

$build = Get-BuildStamp

$image = Build-Image luna-linuxcheck linux-check.Dockerfile

# Single-quoted: the container's shell owns every $ in here.
#
# Every checkout shares the target volume under the same /app path, and cargo goes
# by file times: a checkout whose sources are older than the last build would pass
# for up to date. So the crate itself is always rebuilt; its dependencies stay
# cached. A volume to each checkout would cost a full build and 6 GB apiece.
#
# That rebuild and the tests it makes are one step, under a lock in the volume
# itself: with another check's build landing between the two, this one would run
# that one's tests and report them as its own. Use-BuildLock cannot promise that:
# off Windows it stops at the user, and a Docker daemon may have several. This lock
# goes with the container, however it ends.
$cmd = ConvertTo-BashScript @'
set -e
exec 9>/target/.linux-check.lock
flock -n 9 || { echo 'another Linux check is using these volumes; waiting for it'; flock 9; }
cd /app/src-tauri
cargo clean --locked -p luna
cargo test --locked --lib --bins
'@

Use-BuildLock {
    # --init: the keeper's tests leave orphans to PID 1, and only a real init
    # reaps them; otherwise they stay zombies and look alive to kill(pid, 0).
    # The tmpfs: tauri-build rewrites gen/schemas, for this platform's view of
    # them, on every build.
    Invoke-Checked 'linux check' {
        docker run --rm --init `
          -e LUNA_BUILD=$build `
          -e LUNA_FLAVOR=test `
          -e CARGO_TARGET_DIR=/target `
          -v "${RepoRoot}:/app:ro" `
          --tmpfs /app/src-tauri/gen `
          -v luna-linux-target:/target `
          -v luna-linux-cargo-registry:/usr/local/cargo/registry `
          -w /app `
          $image `
          bash -c $cmd
    }
}
