#Requires -Version 7.4
# Builds the crate for Linux in Docker and runs its whole unit test suite there:
# the Unix code (keeper.rs, parts of procs.rs and pty/) that test-build.ps1
# -Test, with its Windows binaries, never compiles.
#
#   ./scripts/linux-check.ps1
#
# The checkout goes in read-only, so the check leaves nothing behind in it. Runs
# under pwsh on any host with Docker. scripts/gate.ps1 runs these tests too, with
# the formatter and clippy before them.
. "$PSScriptRoot/lib.ps1"

$build = Get-BuildStamp

Use-BuildLock {
    $image = Build-Image luna-linuxcheck linux-check.Dockerfile
    # Single-quoted: the container's shell owns every $ in here. Why the crate is
    # rebuilt each time: Invoke-LinuxCargo.
    Invoke-LinuxCargo 'linux check' $build $image @'
cargo clean --locked -p luna
cargo test --locked --lib --bins
'@
}
